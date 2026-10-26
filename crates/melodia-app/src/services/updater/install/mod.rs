//! Download, verify, and swap or package-install.
//!
//! **The verify completes before any rename touches the live binary path and
//! before any package-manager subprocess runs.** On failure the downloaded file
//! is removed and the live binary is left untouched.
//!
//! Split by stage: [`staging`] picks the path and fingerprints a partial
//! download, [`download`] streams it, [`verify`] checks the signature and runs
//! the post-swap smoke test, [`swap`] does the swap or the elevated install.

mod download;
mod staging;
mod swap;
mod verify;

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use melodia_core::error::{AppError, AppResult};

use super::event::UpdateResult;
use super::manifest::PlatformAsset;
use melodia_platform::services::platform::install_kind::install_target;

use download::download_to_file;
use staging::{
    InstallMethod, resolve_install_method, resolve_staged_path, sidecar_meta_path, staged_msi_path,
    staged_package_path,
};
use swap::{install_via_msiexec, install_via_package_manager};
use verify::{attempt_post_swap_rollback, verify_staged, verify_swapped_binary};

pub use staging::prune_stale_staging;
pub use swap::swap_in_place;

// Re-exported so `super::install_target_old` reaps a stale `.old` at startup through the swap's
// own derivation. Linux-only because that is the only install producing one: Windows flows
// through msiexec, and the one suite asserting the sibling there mounts under `swap`, so it
// reaches `swap::old_path` rather than this.
#[cfg(target_os = "linux")]
pub(crate) use swap::old_path;

/// The version an install swapped in this session, which the next launch runs.
static INSTALLED: Mutex<Option<String>> = Mutex::new(None);

/// Up while Windows Installer is replacing this binary. Restart Manager closes the window the way
/// a user would, and a window hidden to the tray instead keeps `Melodia.exe` in use, which defers
/// the replace to a reboot.
static INSTALLER_RUNNING: AtomicBool = AtomicBool::new(false);

/// What an install that didn't fail came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The next launch runs the new version.
    Installed,
    /// The elevation prompt was dismissed before anything was replaced.
    Cancelled,
}

/// The version an update check measures against: one this session already installed, else this
/// build's own. Measured against the running build instead, a check after an install reads the
/// release it just swapped in as new and offers it again.
pub fn installed_version() -> String {
    INSTALLED.lock().clone().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

/// Whether a window close should quit outright, the installer waiting on this process to exit.
pub fn installer_running() -> bool {
    INSTALLER_RUNNING.load(Ordering::Acquire)
}

/// [`INSTALLER_RUNNING`], lowered on every way out of the wait.
#[cfg(target_os = "windows")]
struct InstallerRunning;

#[cfg(target_os = "windows")]
impl InstallerRunning {
    fn raise() -> Self {
        INSTALLER_RUNNING.store(true, Ordering::Release);
        Self
    }
}

#[cfg(target_os = "windows")]
impl Drop for InstallerRunning {
    fn drop(&mut self) {
        INSTALLER_RUNNING.store(false, Ordering::Release);
    }
}

/// Stream-download `asset.url`, stream-verify it against `asset.signature`,
/// then install it. Calls `on_progress` with a 0..=100 percentage per chunk.
///
/// Any error before the swap removes the partial file; the live binary is only
/// touched once verification passes.
pub async fn download_and_install(
    http: &reqwest::Client,
    asset: &PlatformAsset,
    expected_version: &str,
    on_progress: impl Fn(u8) + Send + Sync,
) -> UpdateResult<InstallOutcome> {
    let target = install_target()?;
    // Housekeeping rather than a step of this install: it collects whatever a *previous* attempt
    // left behind once the retention window closed, and runs here because an install attempt is
    // the moment the staging dir is known to matter.
    prune_stale_staging().await;
    let outcome =
        download_and_install_to(http, asset, expected_version, target, on_progress).await?;
    if outcome == InstallOutcome::Installed {
        *INSTALLED.lock() = Some(expected_version.to_owned());
    }
    Ok(outcome)
}

/// The sequencing, against a target it is handed. Split from the host lookup so a test can drive
/// the failure path without the only target available to it being its own running binary — which
/// is precisely the file a regression to rename-before-verify would replace.
async fn download_and_install_to(
    http: &reqwest::Client,
    asset: &PlatformAsset,
    expected_version: &str,
    target: std::path::PathBuf,
    on_progress: impl Fn(u8) + Send + Sync,
) -> UpdateResult<InstallOutcome> {
    let method = resolve_install_method();
    let staged = match method {
        InstallMethod::LinuxPackage(format) => staged_package_path(&asset.url, format)?,
        InstallMethod::WindowsMsi => staged_msi_path(&asset.url)?,
        InstallMethod::AtomicSwap => resolve_staged_path(&target)?,
    };

    download_to_file(http, &asset.url, expected_version, asset.size, &staged, &on_progress).await?;

    // The stream-verify of a multi-MB file goes on the blocking pool, as does every install but
    // the MSI's.
    let verify_staged_path = staged.clone();
    let verify_signature = asset.signature.clone();
    let verify_version = expected_version.to_string();
    let verify_result = tokio::task::spawn_blocking(move || {
        verify_staged(&verify_staged_path, &verify_signature, &verify_version)
    })
    .await
    .map_err(|e| AppError::updater("update verify task join error", e))?;
    if let Err(e) = verify_result {
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(sidecar_meta_path(&staged));
        return Err(e);
    }

    let outcome = match method {
        InstallMethod::LinuxPackage(format) => {
            let staged = staged.clone();
            install_blocking(move || install_via_package_manager(format, &staged)).await?
        }
        InstallMethod::AtomicSwap => {
            let (target, staged) = (target.clone(), staged.clone());
            install_blocking(move || swap_in_place(&target, &staged)).await?
        }
        // Awaited on the runtime: Restart Manager closing this process partway through is how
        // the wait normally ends, and a pool thread parked under it would buy nothing.
        InstallMethod::WindowsMsi => install_via_msiexec(&staged).await?,
    };
    if outcome == InstallOutcome::Cancelled {
        // The sidecar stays with the verified bytes, so a retry reuses them rather than
        // re-downloading.
        return Ok(outcome);
    }

    // Dropped *before* the smoke test, so a rollback doesn't leak it — a later
    // prune must not read it as leftover state, and the next attempt's
    // `discard_staging_if_sidecar_mismatches` must not be fed a phantom
    // fingerprint. The staged bytes themselves are each method's own business:
    // renamed away, removed after a successful install, or left for the pruner
    // because msiexec cached its own copy.
    let _ = std::fs::remove_file(sidecar_meta_path(&staged));

    // The smoke test is cheap defence against what a signature check can't
    // catch: a successful rename onto a file the kernel refuses to exec. Failure
    // rolls back from the retained `.old`.
    //
    // Skipped on both package paths, for three symmetric reasons. There is no
    // in-process rollback once the package manager has started — the bytes are
    // its, not a `.old` we could rename back. Each format is journaled and
    // recoverable out of band, better than anything a failure here could do. And
    // a blocking subprocess immediately after an elevation prompt makes the
    // spinner look hung. (They verify their own signatures, but none of them runs
    // `Melodia --version`, so that is not the reason.)
    match method {
        InstallMethod::AtomicSwap => {
            if let Err(e) = verify_swapped_binary(&target, expected_version).await {
                attempt_post_swap_rollback(&target);
                return Err(e.into());
            }
        }
        InstallMethod::LinuxPackage(_) => log::info!(
            "updater: package-manager install completed; skipping in-app smoke test \
             (no in-process rollback exists once dnf/apt consumed the staged bytes)"
        ),
        InstallMethod::WindowsMsi => log::info!(
            "updater: msiexec finished; skipping in-app smoke test \
             (Windows Installer owns the replace and keeps its own rollback)"
        ),
    }

    Ok(outcome)
}

/// Runs a synchronous install step on the blocking pool.
async fn install_blocking(
    install: impl FnOnce() -> AppResult<InstallOutcome> + Send + 'static,
) -> AppResult<InstallOutcome> {
    tokio::task::spawn_blocking(install)
        .await
        .map_err(|e| AppError::updater("update install task join error", e))?
}

// Linux-only: every assertion is about the atomic-swap path, which is the method a cargo-built
// binary resolves to. On Windows the same code resolves to msiexec and stages into the real user
// cache dir, so there is nothing here to drive without a second seam nobody needs.
#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/install_tests.rs"]
mod tests;
