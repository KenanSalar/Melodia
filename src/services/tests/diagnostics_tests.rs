use std::path::Path;

use super::{crash_block, settings_block, tail_of};
use crate::error::AppError;
use crate::test_support::{paths_in, reading_env, with_env_var};

/// A file of `lines` numbered lines, each padded to a known width so a byte
/// budget maps onto a predictable number of them.
fn write_numbered(path: &Path, lines: usize) -> Result<(), AppError> {
    let body: Vec<String> = (0..lines).map(|n| format!("line {n:04}\n")).collect();
    std::fs::write(path, body.concat())?;
    Ok(())
}

/// The tail is what a reporter needs — the end of the run, not its start.
#[test]
fn a_tail_returns_the_end_of_the_file() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("melodia_rCURRENT.log");
    write_numbered(&path, 1000)?;

    let tail = reading_env(|| tail_of(&path, 100)).unwrap_or_default();

    assert!(tail.contains("line 0999"), "tail lost the last line");
    assert!(!tail.contains("line 0000"), "tail reached the first line");
    assert!(tail.len() <= 100, "tail blew its budget: {}", tail.len());
    Ok(())
}

/// A budget cuts mid-line, and half a log record reads like a whole one. The
/// partial head has to go.
#[test]
fn a_tail_starts_on_a_line_boundary() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("melodia_rCURRENT.log");
    write_numbered(&path, 1000)?;

    // 25 bytes lands inside a record, never on its boundary.
    let tail = reading_env(|| tail_of(&path, 25)).unwrap_or_default();

    for line in tail.lines() {
        assert!(
            line.starts_with("line "),
            "tail kept a partial record: {line:?}"
        );
    }
    Ok(())
}

/// A file smaller than the budget is returned whole — the boundary trim must
/// not eat its first line.
#[test]
fn a_short_file_keeps_its_first_line() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("small.log");
    write_numbered(&path, 3)?;

    let tail = reading_env(|| tail_of(&path, 64 * 1024)).unwrap_or_default();

    assert!(tail.contains("line 0000"), "trim ate the first line");
    assert!(tail.contains("line 0002"));
    Ok(())
}

/// The bundle goes into a public issue. A home directory usually holds a real
/// name, so it may not survive the trip — in the log body or the file header.
#[test]
fn the_tail_redacts_the_home_directory() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("melodia_rCURRENT.log");
    std::fs::write(&path, b"WARN scan failed for /home/testuser/Music/x.flac\n")?;

    let tail = with_env_var("HOME", Some("/home/testuser"), || tail_of(&path, 64 * 1024))
        .unwrap_or_default();

    assert!(!tail.contains("/home/testuser"), "home leaked: {tail}");
    assert!(tail.contains("~/Music/x.flac"));
    Ok(())
}

/// Most runs never panic, and a bundle from a healthy install still has to
/// build.
#[test]
fn an_install_that_never_crashed_still_reports() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let block = reading_env(|| crash_block(tmp.path()));
    assert!(block.contains("<none>"));
    Ok(())
}

/// The whole point of the allowlist: the block names the fields it was written
/// to name, and no others. The mutation to catch is someone reaching for a
/// whole-struct `Debug` dump, which would pull in the window geometry, the
/// per-theme preference map and — the reason this test exists — anything a
/// later release adds to `SettingsData` without thinking about this file.
#[test]
fn the_settings_block_is_an_allowlist() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let paths = paths_in(tmp.path());

    let block = reading_env(|| settings_block(&paths));

    for expected in ["theme", "locale", "titlebar", "tray", "crossfade"] {
        assert!(block.contains(expected), "block is missing {expected:?}");
    }
    for forbidden in [
        "window_x",
        "window_y",
        "sidebar_width",
        "theme_preferences",
        "eq_band_gains",
        "last_manifest_etag",
        "scrobble",
        "session_key",
        "password",
    ] {
        assert!(
            !block.contains(forbidden),
            "block leaked {forbidden:?}:\n{block}"
        );
    }
    Ok(())
}
