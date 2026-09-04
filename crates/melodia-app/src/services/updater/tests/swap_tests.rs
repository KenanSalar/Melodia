use std::fs;

use tempfile::tempdir;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::old_path;
use super::swap_in_place;
use crate::services::updater::install::staging::staged_path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn same_dir_swap_succeeds() -> TestResult {
    // Regression for the cross-filesystem `$TEMP` → exe-dir rename bug
    // from the v1 plan. By staying inside one `tempdir()`, both files
    // share a filesystem and the rename must succeed.
    let dir = tempdir()?;
    let target = dir.path().join("melodia");
    let staged = staged_path(&target);
    fs::write(&target, b"OLD BINARY BYTES")?;
    fs::write(&staged, b"NEW BINARY BYTES")?;

    swap_in_place(&target, &staged)?;

    let contents = fs::read(&target)?;
    assert_eq!(contents, b"NEW BINARY BYTES");
    assert!(!staged.exists(), "staged file should be consumed by rename");
    Ok(())
}

#[test]
fn swap_replaces_existing_target_bytes() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia.exe");
    let staged = staged_path(&target);
    fs::write(&target, b"v0.1.0")?;
    fs::write(&staged, b"v0.2.0")?;
    swap_in_place(&target, &staged)?;
    assert_eq!(fs::read(&target)?, b"v0.2.0");
    Ok(())
}

/// `swap_in_place` (Linux + Windows branches) keeps a `.old` snapshot of
/// the previously-running binary so a failed post-swap smoke test can
/// roll back, and so `main()`'s startup remove on first successful boot
/// has something to reap. The same-fs atomic rename path is what the
/// per-user tarball / `AppImage` / Windows-zip cases all hit.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn swap_retains_old_snapshot_for_rollback() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    let staged = staged_path(&target);
    let old = old_path(&target);
    fs::write(&target, b"v0.1.0 OLD")?;
    fs::write(&staged, b"v0.2.0 NEW")?;
    assert!(!old.exists(), "test setup: .old must not pre-exist");

    swap_in_place(&target, &staged)?;

    assert_eq!(fs::read(&target)?, b"v0.2.0 NEW", "target carries new bytes");
    assert!(!staged.exists(), "staged is consumed by rename");
    assert!(old.exists(), ".old snapshot is retained for rollback");
    assert_eq!(fs::read(&old)?, b"v0.1.0 OLD", ".old snapshot carries previous-binary bytes");
    Ok(())
}

/// A stale `.old` from a prior incomplete swap shouldn't block a fresh
/// swap; `linux_swap` / `windows_swap` both clear it best-effort before
/// the new two-step rename.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn swap_clears_stale_old_before_retaining_fresh_snapshot() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    let staged = staged_path(&target);
    let old = old_path(&target);
    fs::write(&target, b"v0.2.0")?;
    fs::write(&staged, b"v0.3.0")?;
    fs::write(&old, b"ANCIENT v0.0.1 LEFTOVER")?;

    swap_in_place(&target, &staged)?;

    assert_eq!(fs::read(&target)?, b"v0.3.0");
    assert_eq!(
        fs::read(&old)?,
        b"v0.2.0",
        ".old carries the previous-step bytes, not the ancient leftover"
    );
    Ok(())
}

/// The branch that decides whether the user is left with a working binary or with none. The
/// first rename has already moved the live binary aside when the second one fails, so the
/// restore is the only thing standing between a failed install and an empty install path.
///
/// Driven by omitting the staged file, which is the one way to fail the second rename without
/// privileges: nothing else in the two-step needs a permission a test can withhold.
#[cfg(target_os = "linux")]
#[test]
fn a_failed_second_rename_puts_the_live_binary_back() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    let staged = staged_path(&target);
    let old = old_path(&target);
    std::fs::write(&target, b"v0.2.0 LIVE")?;
    assert!(!staged.exists(), "test setup: the staged file must be absent");

    let outcome = swap_in_place(&target, &staged);

    assert!(outcome.is_err(), "a missing staged file cannot succeed");
    assert_eq!(
        std::fs::read(&target)?,
        b"v0.2.0 LIVE",
        "the live binary must be back at its own path"
    );
    assert!(!old.exists(), "and not left behind as a snapshot of a swap that never happened");
    Ok(())
}

/// The restore is best-effort and logged; the error the caller sees has to stay the one that
/// actually stopped the install, since that is what `FailureKind::classify` reads.
#[cfg(target_os = "linux")]
#[test]
fn the_reported_error_is_the_rename_failure_not_the_rollback() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    std::fs::write(&target, b"v0.2.0 LIVE")?;

    let outcome = swap_in_place(&target, &staged_path(&target));

    let Err(melodia_core::error::AppError::Io(err)) = outcome else {
        return Err(format!("expected the staged rename's io error, got {outcome:?}").into());
    };
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "got {err:?}");
    Ok(())
}
