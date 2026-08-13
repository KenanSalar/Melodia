//! Display + `From<>` coverage for `AppError`.
//!
//! The Tauri version's `Serialize` impl + `kind()` / `inner_message()` helpers
//! went away with the IPC layer, so tests exercising those are intentionally
//! absent.

use super::*;

#[test]
fn display_io() {
    let err = AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
    assert_eq!(format!("{err}"), "IO error: gone");
}

#[test]
fn display_metadata() {
    let err = AppError::metadata_msg("bad tag");
    assert_eq!(format!("{err}"), "Metadata error: bad tag");
}

#[test]
fn display_all_string_variants() {
    let cases: Vec<(AppError, &str)> = vec![
        (AppError::scanner_msg("s"), "Scanner error: s"),
        (AppError::NotFound("n".into()), "Not found: n"),
        (AppError::Player("p".into()), "Player error: p"),
        (AppError::Queue("q".into()), "Queue error: q"),
        (AppError::Settings("st".into()), "Settings error: st"),
        (AppError::Window("w".into()), "Window error: w"),
        (AppError::watcher_msg("wa"), "Watcher error: wa"),
        (AppError::network_msg("ne"), "Network error: ne"),
        (AppError::Validation("v".into()), "Validation error: v"),
    ];
    for (err, expected) in cases {
        assert_eq!(format!("{err}"), expected);
    }
}

#[test]
fn wrapping_constructors_preserve_source_chain() {
    use std::error::Error;

    // A variant built from a message only exposes no source.
    let msg_only = AppError::network_msg("no cause here");
    assert!(msg_only.source().is_none());

    // A variant built by wrapping keeps the typed cause reachable via
    // `.source()` while its own Display carries just the context message.
    let cause = std::io::Error::new(std::io::ErrorKind::TimedOut, "connection timed out");
    let wrapped = AppError::network("Deezer API request failed", cause);
    assert_eq!(format!("{wrapped}"), "Network error: Deezer API request failed");
    assert!(
        wrapped.source().is_some_and(|s| s.to_string().contains("connection timed out")),
        "wrapped error must expose the underlying cause via .source()"
    );

    // `io_source` likewise preserves the wrapped error under the `Io` variant.
    let join_like = std::io::Error::other("task panicked");
    let io_wrapped = AppError::io_source(join_like);
    assert!(io_wrapped.source().is_some());
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
