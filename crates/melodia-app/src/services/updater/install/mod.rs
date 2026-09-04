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

use melodia_core::error::{AppError, AppResult};

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

// Re-exported for two reasons: `super::install_target_old` needs the swap's own
// derivation to reap a stale `.old` at startup, and the Windows swap tests need
// it to assert the sibling. Windows *production* never produces one — installs
// flow through msiexec — so gating it behind `cfg(test)` there keeps the lib
// build's unused-import lint clean without losing the coverage.
#[cfg(any(target_os = "linux", all(test, target_os = "windows")))]
pub(crate) use swap::old_path;

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
) -> AppResult<()> {
    let target = install_target()?;
    // Housekeeping rather than a step of this install: it collects whatever a *previous* attempt
    // left behind once the retention window closed, and runs here because an install attempt is
    // the moment the staging dir is known to matter.
    prune_stale_staging().await;
    download_and_install_to(http, asset, expected_version, target, on_progress).await
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
) -> AppResult<()> {
    let method = resolve_install_method();
    let staged = match method {
        InstallMethod::LinuxPackage(format) => staged_package_path(&asset.url, format)?,
        InstallMethod::WindowsMsi => staged_msi_path(&asset.url)?,
        InstallMethod::AtomicSwap => resolve_staged_path(&target)?,
    };

    download_to_file(http, &asset.url, expected_version, asset.size, &staged, &on_progress).await?;

    // Both blocking phases — the stream-verify of a multi-MB file, then whichever
    // install the method picks — go on the blocking pool.
    let verify_staged_path = staged.clone();
    let verify_signature = asset.signature.clone();
    let verify_version = expected_version.to_string();
    let verify_result = tokio::task::spawn_blocking(move || {
        verify_staged(&verify_staged_path, &verify_signature, &verify_version)
    })
    .await
    .map_err(|e| AppError::Settings(format!("update verify task join error: {e}")))?;
    if let Err(e) = verify_result {
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(sidecar_meta_path(&staged));
        return Err(e);
    }

    let install_staged = staged.clone();
    let install_target_path = target.clone();
    let install_result = tokio::task::spawn_blocking(move || match method {
        InstallMethod::LinuxPackage(format) => install_via_package_manager(format, &install_staged),
        InstallMethod::WindowsMsi => install_via_msiexec(&install_staged),
        InstallMethod::AtomicSwap => swap_in_place(&install_target_path, &install_staged),
    })
    .await
    .map_err(|e| AppError::Settings(format!("update install task join error: {e}")))?;
    install_result?;

    // Dropped *before* the smoke test, so a rollback doesn't leak it — a later
    // prune must not read it as leftover state, and the next attempt's
    // `discard_staging_if_sidecar_mismatches` must not be fed a phantom
    // fingerprint. The staged bytes themselves are each method's own business:
    // renamed away, removed after a successful install, or left for the pruner
    // because msiexec may still be reading them.
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
    // spinner look hung; on Windows it is racy besides, msiexec possibly being
    // mid-replace. (They verify their own signatures, but none of them runs
    // `Melodia --version`, so that is not the reason.)
    match method {
        InstallMethod::AtomicSwap => {
            if let Err(e) = verify_swapped_binary(&target, expected_version).await {
                attempt_post_swap_rollback(&target);
                return Err(e);
            }
        }
        InstallMethod::LinuxPackage(_) => log::info!(
            "updater: package-manager install completed; skipping in-app smoke test \
             (no in-process rollback exists once dnf/apt consumed the staged bytes)"
        ),
        InstallMethod::WindowsMsi => log::info!(
            "updater: msiexec dispatched; skipping in-app smoke test \
             (Windows Installer owns the replace; running the new binary now would \
             race the in-progress swap)"
        ),
    }

    Ok(())
}

// Linux-only: every assertion is about the atomic-swap path, which is the method a cargo-built
// binary resolves to. On Windows the same code resolves to msiexec and stages into the real user
// cache dir, so there is nothing here to drive without a second seam nobody needs.
#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/install_tests.rs"]
mod tests;
