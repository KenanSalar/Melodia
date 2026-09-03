//! Display + `From<>` coverage for `AppError`.
//!
//! The Tauri version's `Serialize` impl + `kind()` / `inner_message()` helpers
//! went away with the IPC layer, so tests exercising those are intentionally
//! absent.

use super::*;
use crate::test_support::{MIN_SOURCES, SRC_DIR, stripped_sources};

/// The two `Display` shapes in this tree, which is what makes `describe` reachable without knowing
/// which one is in hand. `Network` names an operation and leaves the cause on `.source()`, so the
/// walk is the whole point; `Io` is `#[error("IO error: {0}")]` over the field `#[from]` also
/// makes the source, so an unconditional walk would print it twice — and sqlx nests that shape.
#[test]
fn a_cause_is_appended_once_and_never_repeated() {
    let denied = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let cause = denied().to_string();

    let with_context = AppError::network("Failed to parse Deezer response", denied());
    assert_eq!(
        describe(&with_context),
        format!("Network error: Failed to parse Deezer response: {cause}"),
        "a context message drops its cause without the walk"
    );

    let interpolated = AppError::Io(denied());
    assert_eq!(
        describe(&interpolated),
        interpolated.to_string(),
        "a `Display` that already prints its source has nothing left to append"
    );
}

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

/// An error carried as a `String` keeps its message and drops its cause, which is the whole of
/// what [`describe`] walks. Nothing under [`SRC_DIR`] needs to: [`AppError`] reaches
/// everywhere, and where it cannot (`radio_blocklist::source` is `include!`d into `build.rs`, so
/// it may name no `crate::` path) the answer is a local type implementing `std::error::Error`.
/// The rule is violable from any file and costs nothing to break, so a walk is what holds it.
#[test]
fn no_result_carries_its_error_as_a_string() {
    let offenders: Vec<String> = stripped_sources(SRC_DIR, "rs", MIN_SOURCES)
        .into_iter()
        .filter(|(_, code)| string_errored_results(code) > 0)
        .map(|(path, _)| path)
        .collect();

    assert!(
        offenders.is_empty(),
        "{offenders:?} carry a failure as a plain `String`. Keep it typed: `AppError`, or a \
         local type implementing `std::error::Error` where `AppError` cannot be named, and \
         flatten to text at the point the error is read"
    );
}

/// How many of `code`'s `Result`s close on a bare `String` error argument.
fn string_errored_results(code: &str) -> usize {
    // In halves so this file isn't its own first hit. The alternative its neighbours reach for,
    // skipping the file that spells the needle, would stop it covering itself.
    const OPENER: &str = concat!("Result", "<");

    let mut found = 0;
    let mut rest = code;
    while let Some(start) = rest.find(OPENER) {
        rest = &rest[start + OPENER.len()..];
        let closed = generic_arguments(rest);
        if closed.and_then(error_argument) == Some("String") {
            found += 1;
        }
    }
    found
}

/// The text up to the `>` closing an already-opened generic list.
///
/// A `>` behind a `-` is a return arrow rather than a close, which a `Fn` bound inside the list
/// would otherwise use to end it early.
fn generic_arguments(after_opener: &str) -> Option<&str> {
    let mut depth = 1_usize;
    let mut previous = ' ';
    for (offset, character) in after_opener.char_indices() {
        match character {
            '<' => depth += 1,
            '>' if previous != '-' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&after_opener[..offset]);
                }
            }
            _ => {}
        }
        previous = character;
    }
    None
}

/// The last of `arguments`' top-level parameters, or `None` when there is only one.
///
/// A single-parameter `Result` is an alias carrying its own error (`io::Result<T>`), which has
/// no error position to judge.
fn error_argument(arguments: &str) -> Option<&str> {
    let mut depth = 0_usize;
    let mut last = None;
    for (offset, character) in arguments.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => last = Some(arguments[offset + 1..].trim()),
            _ => {}
        }
    }
    last
}
