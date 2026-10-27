//! The staged file's signature check, the post-swap `--version` smoke test, and the rollback from
//! `.old` when that fails.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::services::updater::event::{UpdateFailure, UpdateResult};
use crate::services::updater::minisign::{self, MinisignError};
use melodia_core::error::{AppError, AppResult};

#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::swap::old_path;

pub(super) fn verify_staged(
    staged: &Path,
    signature: &str,
    expected_version: &str,
) -> UpdateResult<()> {
    let f = File::open(staged).map_err(|e| AppError::io("opening the staged update failed", e))?;
    let reader = BufReader::new(f);
    let pubkey = minisign::embedded_pubkey()
        .map_err(|e| AppError::updater("embedded updater pubkey is invalid", e))?;
    minisign::verify_stream(reader, signature, &pubkey, Some(expected_version))
        .map_err(verification_failure)
}

fn verification_failure(e: MinisignError) -> UpdateFailure {
    match e {
        // Failing to read the staged file says nothing about its signature.
        MinisignError::Io(e) => AppError::io("reading the staged update failed", e).into(),
        e => UpdateFailure::signature(AppError::updater("update signature verification failed", e)),
    }
}

/// Requires `target --version` to exit 0 within 5 s printing `Melodia <expected_version>`, the
/// contract `main()`'s literal first branch keeps.
///
/// The signature already vouched for the bytes, so this checks only that the kernel runs them as
/// this release. A healthy binary answers in milliseconds; 5 s covers a cold cache on a loaded,
/// low-power ARM board without leaving the Install spinner up long for a broken one.
pub(super) async fn verify_swapped_binary(target: &Path, expected_version: &str) -> AppResult<()> {
    use tokio::time::{Duration, timeout};

    log::info!(
        "updater: smoke-testing swapped binary ({} --version, expecting Melodia {expected_version})",
        target.display()
    );

    let cmd = tokio::process::Command::new(target).arg("--version").output();
    let output = match timeout(Duration::from_secs(5), cmd).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(AppError::updater(
                format!(
                    "post-swap launch verification: failed to spawn {} --version",
                    target.display()
                ),
                e,
            ));
        }
        Err(_elapsed) => {
            return Err(AppError::updater_msg(format!(
                "post-swap launch verification timed out: {} --version did not reply within 5s",
                target.display()
            )));
        }
    };

    if !output.status.success() {
        return Err(AppError::updater_msg(format!(
            "post-swap launch verification: binary exited {} ({})",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if !trimmed.starts_with("Melodia ") || !trimmed.contains(expected_version) {
        return Err(AppError::updater_msg(format!(
            "post-swap launch verification: expected `Melodia <version>` containing \
             {expected_version}, got: {trimmed}"
        )));
    }

    log::info!("updater: smoke test passed: {trimmed}");
    Ok(())
}

/// Restores `.old` over a binary that failed its smoke test. The `pkexec mv` path keeps no `.old`,
/// leaving nothing to restore.
pub(super) fn attempt_post_swap_rollback(target: &Path) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let old = old_path(target);
        if !old.exists() {
            log::warn!(
                "updater: smoke test failed but no .old snapshot exists at {}; nothing to \
                 rollback (likely the pkexec-mv path was taken). User may need to manually \
                 reinstall the previous version.",
                old.display()
            );
            return;
        }
        // A missing binary beats a broken one posing as a valid install.
        let _ = std::fs::remove_file(target);
        match std::fs::rename(&old, target) {
            Ok(()) => log::info!(
                "updater: rolled back swap — restored {} from .old snapshot",
                target.display()
            ),
            Err(e) => log::warn!(
                "updater: rollback failed: could not rename {} → {}: {}",
                old.display(),
                target.display(),
                melodia_core::error::describe(&e)
            ),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = target;
        log::warn!("updater: smoke test failed; rollback not implemented on this platform");
    }
}

// Linux-only: the stand-in for a swapped binary is a shell script, and the rollback half
// reaches `old_path`, which only exists on the two platforms that retain one.
#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/verify_tests.rs"]
mod tests;
