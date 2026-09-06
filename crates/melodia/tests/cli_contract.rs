//! The two literal-first branches in `main()`, pinned against the binary cargo just built.
//!
//! `--version` is a forward-compatibility contract rather than a convenience: the updater's
//! post-swap smoke test spawns the *new* binary with it and rolls the update back unless it
//! answers, so a client that regresses it strands every install already in the field on the
//! version it has. That makes this the one place in the suite worth spawning a process for —
//! nothing about the branch can be pinned by reading source, because what the verifier reads is
//! the binary's behaviour.

use std::path::PathBuf;
use std::process::Command;

/// Cargo spells this after the artifact, which is deliberately not the package name — see the
/// `[[bin]]` block in this package's manifest.
const MELODIA: &str = env!("CARGO_BIN_EXE_Melodia");

/// What `install::verify::verify_swapped_binary` asserts before it discards the old binary.
#[test]
fn the_version_branch_answers_what_the_post_swap_verifier_reads() -> std::io::Result<()> {
    let output = Command::new(MELODIA).arg("--version").output()?;

    assert!(
        output.status.success(),
        "`--version` exited {:?}; the verifier treats a non-zero exit as a failed update and \
         rolls back",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with("Melodia "),
        "the verifier prefix-matches `Melodia `, got {trimmed:?}"
    );
    assert_eq!(trimmed, format!("Melodia {}", env!("CARGO_PKG_VERSION")));
    Ok(())
}

/// The branch sits ahead of `Paths::resolve` for this reason, and an unusable data directory is
/// the cheapest way to prove it still does: anything that can fail moved above it would take the
/// smoke test down with it on exactly the installs least able to recover.
#[test]
fn the_version_branch_answers_before_anything_that_can_fail() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let not_a_directory = tmp.path().join("occupied");
    std::fs::write(&not_a_directory, b"")?;

    let output = Command::new(MELODIA)
        .arg("--version")
        .env("MELODIA_DATA_DIR", &not_a_directory)
        .output()?;

    assert!(
        output.status.success(),
        "`--version` must not depend on a usable data directory; exited {:?}",
        output.status.code()
    );
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Melodia "));
    Ok(())
}

/// `--logs` answers when the thing being reported is that Melodia won't open, so it owes the same
/// ordering and the same freedom from the database and Slint. Steered onto a tempdir because the
/// alternative is asserting against whatever the developer's real install holds.
#[test]
fn the_logs_branch_prints_the_resolved_logs_dir() -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let root: PathBuf = std::path::absolute(tmp.path())?.components().collect();

    let output = Command::new(MELODIA).arg("--logs").env("MELODIA_DATA_DIR", &root).output()?;

    assert!(output.status.success(), "`--logs` exited {:?}", output.status.code());

    let printed = String::from_utf8_lossy(&output.stdout);
    assert_eq!(PathBuf::from(printed.trim()), root.join("logs"));
    assert!(root.join("logs").is_dir(), "`--logs` resolves paths, which creates them");
    Ok(())
}
