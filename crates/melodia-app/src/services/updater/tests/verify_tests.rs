//! The two halves of what happens after the rename: the smoke test that decides whether the new
//! binary is kept, and the rollback that runs when it isn't.
//!
//! `cli_contract.rs` pins the producing side of the `--version` contract against the real binary.
//! This is the reading side, and until both are pinned a reword could satisfy one and break the
//! other. The stand-in here is a shell script rather than a binary because what the verifier
//! actually consumes is an exit code and a line of stdout.
//!
//! Linux-only for that reason — the script is the double, and a `.bat` would be a second one to
//! keep true. The 5 s timeout stays untested on purpose: a test for it either sleeps through the
//! budget or re-asserts the constant against itself.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use melodia_core::error::AppError;
use tempfile::{TempDir, tempdir};

use super::{attempt_post_swap_rollback, old_path, verify_swapped_binary};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A stand-in for the freshly swapped binary, answering `--version` however the case needs.
fn fake_binary(dir: &Path, body: &str) -> std::io::Result<PathBuf> {
    let path = dir.join("Melodia");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

fn refusal(outcome: Result<(), AppError>) -> Result<String, Box<dyn std::error::Error>> {
    match outcome {
        Err(AppError::Validation(msg)) => Ok(msg),
        other => Err(format!("expected a Validation refusal, got {other:?}").into()),
    }
}

/// The acceptance condition for every in-place update: exit 0, the `Melodia ` prefix, and the
/// version the manifest promised somewhere in the line.
#[tokio::test]
async fn a_binary_that_answers_with_its_version_passes_the_smoke_test() -> TestResult {
    let dir = tempdir()?;
    let binary = fake_binary(dir.path(), "echo 'Melodia 0.3.0'")?;

    verify_swapped_binary(&binary, "0.3.0").await?;
    Ok(())
}

/// A binary that starts and fails is the case the smoke test exists for — a signature check
/// cannot tell you the kernel will refuse to run what it just verified.
#[tokio::test]
async fn a_nonzero_exit_fails_the_smoke_test() -> TestResult {
    let dir = tempdir()?;
    let binary = fake_binary(dir.path(), "echo 'Melodia 0.3.0'; exit 3")?;

    let msg = refusal(verify_swapped_binary(&binary, "0.3.0").await)?;
    assert!(msg.contains("exited 3"), "the exit code is what a bug report needs: {msg}");
    Ok(())
}

/// The reading half of the `--version` contract. Each of these is a way the producing side could
/// drift — a dropped prefix, a case change, a version that is not the one being installed — and
/// each has to be refused rather than accepted as close enough.
#[tokio::test]
async fn output_that_misses_the_contract_fails_the_smoke_test() -> TestResult {
    let cases = [
        ("echo ''", "no output at all"),
        ("echo 'melodia 0.3.0'", "a lowercased prefix"),
        ("echo 'Melodia-0.3.0'", "the space dropped from the prefix"),
        ("echo 'Melodia 0.2.0'", "the version that was replaced, not the one installed"),
        ("echo 'Melodia 0.3.0' >&2", "the line on stderr rather than stdout"),
    ];

    for (body, what) in cases {
        let dir = tempdir()?;
        let binary = fake_binary(dir.path(), body)?;
        let outcome = verify_swapped_binary(&binary, "0.3.0").await;
        assert!(outcome.is_err(), "{what} must not pass: {outcome:?}");
    }
    Ok(())
}

/// A target that will not execute at all. Reported as its own case because the message is what
/// tells a user whether the file arrived or the file is broken.
#[tokio::test]
async fn a_target_that_cannot_be_spawned_fails_the_smoke_test() -> TestResult {
    let dir = tempdir()?;
    let missing = dir.path().join("Melodia");

    let msg = refusal(verify_swapped_binary(&missing, "0.3.0").await)?;
    assert!(msg.contains("failed to spawn"), "{msg}");
    Ok(())
}

/// Seeds the pair a rollback acts on: a swapped-in binary and the snapshot it displaced.
fn swapped_with_snapshot() -> std::io::Result<(TempDir, PathBuf)> {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    std::fs::write(&target, b"v0.3.0 BROKEN")?;
    std::fs::write(old_path(&target), b"v0.2.0 WORKING")?;
    Ok((dir, target))
}

/// The whole point of retaining `.old`: a user whose new binary will not start gets the one that
/// did back, without being told to reinstall by hand.
#[test]
fn a_rollback_restores_the_retained_snapshot() -> TestResult {
    let (_dir, target) = swapped_with_snapshot()?;

    attempt_post_swap_rollback(&target);

    assert_eq!(std::fs::read(&target)?, b"v0.2.0 WORKING", "the working binary must be back");
    assert!(!old_path(&target).exists(), "and consumed, so the next boot's reaper finds nothing");
    Ok(())
}

/// The `pkexec mv` path retains no snapshot, so a rollback there has nothing to restore. It must
/// leave what is on disk alone rather than removing the binary and replacing it with nothing.
#[test]
fn a_rollback_with_no_snapshot_leaves_the_target_where_it_is() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    std::fs::write(&target, b"v0.3.0 BROKEN")?;
    assert!(!old_path(&target).exists(), "test setup: no snapshot");

    attempt_post_swap_rollback(&target);

    assert!(target.exists(), "a broken binary still beats no binary");
    assert_eq!(std::fs::read(&target)?, b"v0.3.0 BROKEN");
    Ok(())
}
