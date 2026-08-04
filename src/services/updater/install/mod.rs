//! Download + verify + atomic swap (or package-manager install).
//!
//! Critical ordering: verify completes before any rename touches the
//! live binary path **and** before any package-manager subprocess
//! runs. On verify failure the downloaded `.new` file is removed and
//! the live binary is left untouched.
//!
//! Split by stage:
//!
//! * [`staging`] — staging-path selection + the partial-download
//!   sidecar fingerprint.
//! * [`download`] — streaming download, HTTP-range resume, size bound.
//! * [`verify`] — minisign signature check + post-swap smoke test +
//!   rollback.
//! * [`swap`] — the atomic in-place swap / elevated package install.

mod download;
mod staging;
mod swap;
mod verify;

use crate::error::{AppError, AppResult};

use super::install_target;
use super::manifest::PlatformAsset;

use download::download_to_file;
use staging::{
    InstallMethod, resolve_install_method, resolve_staged_path, sidecar_meta_path,
    staged_msi_path, staged_package_path,
};
use swap::{install_via_msiexec, install_via_package_manager};
use verify::{attempt_post_swap_rollback, verify_staged, verify_swapped_binary};

pub use staging::prune_stale_staging;
pub use swap::swap_in_place;

// `old_path` derives `<binary>.old`; re-exported because:
//   * Linux production: `super::install_target_old` (called from
//     `main.rs` at startup) needs the exact same path-derivation
//     logic the swap uses to reap a successful boot's stale `.old`.
//   * Windows tests: `install_tests::swap_retains_old_snapshot_*` /
//     `swap_clears_stale_old_*` exercise `windows_swap` and need
//     `old_path` to assert the `.old` sibling.
//
// Windows production doesn't need it — installs flow through
// `msiexec /i` of a signed MSI (no `.old` ever produced at the
// install target). Gating Windows behind `cfg(test)` keeps the lib
// build's unused-imports lint clean while preserving test coverage of
// the swap helpers.
#[cfg(any(target_os = "linux", all(test, target_os = "windows")))]
pub(crate) use swap::old_path;

/// Stream-download `asset.url` to a sibling of the install target,
/// stream-verify the downloaded file against `asset.signature`, then
/// atomically swap it over the live binary. Calls `on_progress` with a
/// 0..=100 percentage on each chunk.
///
/// On any error before swap, the partially-downloaded `.new` file is
/// removed. The live binary is only touched once verification passes.
pub async fn download_and_install(
    http: &reqwest::Client,
    asset: &PlatformAsset,
    expected_version: &str,
    on_progress: impl Fn(u8) + Send + Sync,
) -> AppResult<()> {
    let target = install_target()?;
    let method = resolve_install_method();
    let staged = match method {
        InstallMethod::LinuxPackage(format) => staged_package_path(&asset.url, format)?,
        InstallMethod::WindowsMsi => staged_msi_path(&asset.url)?,
        InstallMethod::AtomicSwap => resolve_staged_path(&target)?,
    };

    // Best-effort cleanup of stale staging artifacts (older than 7d)
    // before we start. Failed / auth-cancelled installs deliberately
    // keep their staged files for a retry; this gathers them up later.
    prune_stale_staging().await;

    download_to_file(http, &asset.url, expected_version, asset.size, &staged, &on_progress).await?;

    // Hand the blocking phases (stream-verify of a multi-MB file, then
    // either an in-place rename / `msiexec /i` spawn / `pkexec dnf
    // install` subprocess) to the blocking pool so the async worker
    // stays free for other tasks.
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

    // Drop the orphan sidecar *before* the smoke test so a
    // rollback-on-smoke-fail path doesn't leak it; a future
    // `prune_stale_staging` pass shouldn't pick it up as leftover
    // state and `discard_staging_if_sidecar_mismatches` on the next
    // attempt shouldn't be fed a phantom fingerprint.
    //
    // The staged bytes themselves are consumed differently per method:
    //   * `AtomicSwap` — renamed away by `swap_in_place`.
    //   * `LinuxPackage` — `install_via_package_manager` removes the
    //     file after a successful `dnf/apt install`.
    //   * `WindowsMsi` — `install_via_msiexec` spawns msiexec
    //     non-blocking; msiexec may still be reading the `.msi` when
    //     this function returns. The file stays on disk until the
    //     7d pruner reaps it (or a successful next-install attempt
    //     finds it stale and replaces it).
    let _ = std::fs::remove_file(sidecar_meta_path(&staged));

    // Smoke-test the newly-installed binary on the atomic-swap path.
    // Cheap defence against the rare class of failures the signature
    // check can't catch: a successful `rename(2)` to a file that the
    // kernel will refuse to exec (broken `interpreter` line on a
    // wrapper, exec-bit lost in an upstream packaging mishap, ABI
    // mismatch with the running kernel, …). Failure rolls back from
    // the retained `.old`.
    //
    // Skipped on the package-manager + Windows MSI paths. The reasons
    // are symmetric across both:
    //   1. No in-process rollback exists once `dnf`/`apt`/`msiexec`
    //      has started — the staged bytes are owned by the package
    //      format, not retained as a `.old` snapshot we could rename
    //      back.
    //   2. Each install format is journaled and recoverable out-of-
    //      band: `dnf history undo`, `apt install <prev-version>`,
    //      or Windows Installer's per-product rollback via Add/Remove
    //      Programs → "Modify". A smoke-test failure here couldn't do
    //      anything the user can't already do better with one of
    //      those.
    //   3. Running a 5 s blocking subprocess immediately after the
    //      elevation prompt (polkit / UAC) makes the post-install
    //      spinner look hung for no actionable benefit. On Windows
    //      it's also racy — msiexec may be mid-replace when we'd try
    //      to exec the new binary, which would either spawn the old
    //      version (mid-rename window) or fail with "file in use".
    //
    // (Note: dnf/apt verify package signature + checksum at install
    // time and Windows Installer enforces the same on the MSI summary
    // stream, but none of them runs `Melodia --version`. So the skip
    // isn't "the package manager already smoke-tested for us" — it's
    // the three reasons above.)
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

#[cfg(test)]
#[path = "../tests/install_tests.rs"]
mod tests;
