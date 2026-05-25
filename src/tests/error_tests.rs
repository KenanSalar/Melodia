//! Display + `From<>` coverage for `AppError`.
//!
//! The Tauri version's `Serialize` impl + `kind()` / `inner_message()` helpers
//! were dropped during Phase 0 (no IPC anymore). Tests that exercised those are
//! intentionally absent.

use super::*;

#[test]
fn display_io() {
    let err = AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
    assert_eq!(format!("{err}"), "IO error: gone");
}

#[test]
fn display_metadata() {
    let err = AppError::Metadata("bad tag".into());
    assert_eq!(format!("{err}"), "Metadata error: bad tag");
}

#[test]
fn display_all_string_variants() {
    let cases: Vec<(AppError, &str)> = vec![
        (AppError::Scanner("s".into()), "Scanner error: s"),
        (AppError::NotFound("n".into()), "Not found: n"),
        (AppError::Player("p".into()), "Player error: p"),
        (AppError::Queue("q".into()), "Queue error: q"),
        (AppError::Settings("st".into()), "Settings error: st"),
        (AppError::Window("w".into()), "Window error: w"),
        (AppError::Watcher("wa".into()), "Watcher error: wa"),
        (AppError::Network("ne".into()), "Network error: ne"),
        (AppError::Validation("v".into()), "Validation error: v"),
    ];
    for (err, expected) in cases {
        assert_eq!(format!("{err}"), expected);
    }
}

#[test]
fn display_database() {
    let err = AppError::Database(sqlx::Error::RowNotFound);
    let msg = format!("{err}");
    assert!(msg.starts_with("Database error:"), "got: {msg}");
}

#[test]
fn from_io_error() {
    let io_err = std::io::Error::other("disk");
    let err: AppError = io_err.into();
    assert!(matches!(err, AppError::Io(_)));
}

#[test]
fn from_sqlx_error() {
    let err: AppError = sqlx::Error::RowNotFound.into();
    assert!(matches!(err, AppError::Database(_)));
}

#[test]
fn not_found_helper_formats() {
    let err = AppError::not_found("track", 42);
    assert_eq!(format!("{err}"), "Not found: track not found: 42");
}

#[test]
fn io_other_helper_wraps_message() {
    let err = AppError::io_other("disk full");
    let msg = format!("{err}");
    assert!(msg.contains("disk full"), "got: {msg}");
    assert!(matches!(err, AppError::Io(_)));
}
