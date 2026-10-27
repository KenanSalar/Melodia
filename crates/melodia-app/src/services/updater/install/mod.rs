//! Download, verify, and swap or package-install. **The signature check completes before anything
//! touches the live binary or runs a package manager**, and a failed one removes the download.

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

// For `super::install_target_old`; Windows installs through msiexec and keeps no `.old`.
#[cfg(target_os = "linux")]
pub(crate) use swap::old_path;

/// The version an install swapped in this session, which the next launch runs.
static INSTALLED: Mutex<Option<String>> = Mutex::new(None);

/// Up while Windows Installer replaces this binary. Restart Manager closes the window as a user
/// would, and one hidden to the tray instead keeps `Melodia.exe` in use until a reboot.
static INSTALLER_RUNNING: AtomicBool = AtomicBool::new(false);

/// What an install that didn't fail came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The next launch runs the new version.
    Installed,
    /// The elevation prompt was dismissed before anything was replaced.
    Cancelled,
}

/// The version a check measures against: one this session installed, else this build's, so a
/// check after an install doesn't offer the same release again.
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

/// Downloads, verifies and installs `asset`, reporting 0..=100 through `on_progress`.
pub async fn download_and_install(
    http: &reqwest::Client,
    asset: &PlatformAsset,
    expected_version: &str,
    on_progress: impl Fn(u8) + Send + Sync,
) -> UpdateResult<InstallOutcome> {
    let target = install_target()?;
    // Housekeeping for earlier attempts, run now that the staging dir is about to matter.
    prune_stale_staging().await;
    let outcome =
        download_and_install_to(http, asset, expected_version, target, on_progress).await?;
    if outcome == InstallOutcome::Installed {
        *INSTALLED.lock() = Some(expected_version.to_owned());
    }
    Ok(outcome)
}

/// Split from the host lookup so a test's target isn't its own running binary, the file a
/// rename-before-verify regression would replace.
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
        // On the runtime: the wait normally ends with this process closed under it.
        InstallMethod::WindowsMsi => install_via_msiexec(&staged).await?,
    };
    if outcome == InstallOutcome::Cancelled {
        // The sidecar stays, so a retry reuses the verified bytes.
        return Ok(outcome);
    }

    // Before the smoke test, so a rollback can't leave a fingerprint behind for bytes that are
    // gone. The staged bytes are each method's own business.
    let _ = std::fs::remove_file(sidecar_meta_path(&staged));

    // The smoke test catches what a signature can't, a binary the kernel won't exec, and rolls
    // back from `.old`. The package paths skip it: once the package manager has started there is
    // no `.old` to restore, and each format keeps its own journal to recover from.
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

async fn install_blocking(
    install: impl FnOnce() -> AppResult<InstallOutcome> + Send + 'static,
) -> AppResult<InstallOutcome> {
    tokio::task::spawn_blocking(install)
        .await
        .map_err(|e| AppError::updater("update install task join error", e))?
}

// Linux-only: on Windows the same code resolves to msiexec and stages into the real cache dir.
#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/install_tests.rs"]
mod tests;
