//! Post-download verification: minisign signature check on the staged
//! bytes, the post-swap `--version` smoke test, and rollback from the
//! retained `.old` snapshot when the smoke test fails.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::services::updater::minisign;
use melodia_core::error::{AppError, AppResult};

#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::swap::old_path;

pub(super) fn verify_staged(
    staged: &Path,
    signature: &str,
    expected_version: &str,
) -> AppResult<()> {
    let f = File::open(staged)?;
    let reader = BufReader::new(f);
    let pubkey = minisign::embedded_pubkey()
        .map_err(|e| AppError::Validation(format!("embedded updater pubkey is invalid: {e}")))?;
    minisign::verify_stream(reader, signature, &pubkey, Some(expected_version))
        .map_err(|e| AppError::Validation(format!("update signature verification failed: {e}")))
}

/// Spawn `target --version` and assert it (a) exits 0 within 5 s and
/// (b) prints `Melodia <expected_version>` to stdout. The `--version`
/// fast path in `main.rs` is the contract this verifier relies on
/// (literal first branch of `main()`, prints `Melodia <CARGO_PKG_VERSION>`).
///
/// Why 5 s: a healthy `--version` exits in milliseconds; 5 s is enough
/// slack for cold-cache disk read + ld.so resolution + glibc startup on
/// a heavily-loaded laptop or a power-constrained aarch64 device (Raspberry
/// Pi 4/5, Ampere developer board, Snapdragon X laptop on battery saver),
/// and short enough that an actually-broken new binary doesn't keep the
/// user waiting on the Install spinner. Earlier 3 s was tight on slower
/// ARM hardware where the rollback path triggered on slow-but-healthy
/// boots.
///
/// Why assert on the stdout shape: signature verification already
/// confirmed the bytes are what we signed, so this is primarily an
/// exec-sanity check (was the swap actually a Melodia binary; can the
/// kernel run it; does it boot far enough to reach main's first
/// statement). Asserting `"Melodia "` + the expected version closes
/// the contrived gap where the swap target somehow ended up pointing
/// at a different, well-formed CLI tool that happens to exit 0 on
/// `--version` — that would pass the previous "non-empty stdout" check.
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
            return Err(AppError::Validation(format!(
                "post-swap launch verification: failed to spawn {} --version: {e}",
                target.display()
            )));
        }
        Err(_elapsed) => {
            return Err(AppError::Validation(format!(
                "post-swap launch verification timed out: {} --version did not reply within 5s",
                target.display()
            )));
        }
    };

    if !output.status.success() {
        return Err(AppError::Validation(format!(
            "post-swap launch verification: binary exited {} ({})",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if !trimmed.starts_with("Melodia ") || !trimmed.contains(expected_version) {
        return Err(AppError::Validation(format!(
            "post-swap launch verification: expected `Melodia <version>` containing \
             {expected_version}, got: {trimmed}"
        )));
    }

    log::info!("updater: smoke test passed: {trimmed}");
    Ok(())
}

/// Try to restore the previous binary if a smoke test fails. Only the
/// per-user atomic-swap paths (`linux_swap` happy branch + `windows_swap`)
/// retain a usable `.old`; the `pkexec mv` branch leaves nothing to
/// roll back to. Logs the outcome either way — silent failure here is
/// the wrong kind of quiet.
///
/// The package-manager path doesn't call this — see `download_and_install`,
/// which skips the smoke test entirely when `pkg_format.is_some()`.
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
        // Drop the failed new binary, restore the snapshot. Best-effort —
        // on failure we log and the user sees a missing binary on next
        // launch. Better than leaving the broken file in place pretending
        // to be a valid install.
        let _ = std::fs::remove_file(target);
        match std::fs::rename(&old, target) {
            Ok(()) => log::info!(
                "updater: rolled back swap — restored {} from .old snapshot",
                target.display()
            ),
            Err(e) => log::warn!(
                "updater: rollback failed: could not rename {} → {}: {e}",
                old.display(),
                target.display()
            ),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = target;
        log::warn!("updater: smoke test failed; rollback not implemented on this platform");
    }
}
