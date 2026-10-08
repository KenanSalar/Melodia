//! Display + `From<>` coverage for `AppError`.
//!
//! The Tauri version's `Serialize` impl + `kind()` / `inner_message()` helpers
//! went away with the IPC layer, so tests exercising those are intentionally
//! absent.

use super::*;

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
fn an_error_with_no_cause_reads_as_its_own_display() {
    let error = AppError::network_msg("Failed to reach the directory");

    assert_eq!(describe(&error), "Network error: Failed to reach the directory");
}

/// Every log line in the tree hands its error here, and the chains it gets run deeper than one
/// level: an integration's error wraps a D-Bus error that wraps an I/O one.
#[test]
fn every_level_of_a_cause_chain_is_appended_in_order() {
    let error = layer(
        "Failed to update the media controls",
        Some(layer("D-Bus call failed", Some(layer("connection reset by peer", None)))),
    );

    assert_eq!(
        describe(&error),
        "Failed to update the media controls: D-Bus call failed: connection reset by peer"
    );
}

/// sqlx's shape one level down: a cause whose `Display` already prints its own source. The skip
/// compares against everything written so far, not against the level directly above.
#[test]
fn a_cause_printed_further_up_the_chain_is_not_repeated() {
    let error = layer(
        "Failed to save the playlist",
        Some(layer(
            "error returned from database: UNIQUE constraint failed",
            Some(layer("UNIQUE constraint failed", None)),
        )),
    );

    assert_eq!(
        describe(&error),
        "Failed to save the playlist: error returned from database: UNIQUE constraint failed"
    );
}

/// The `Io` variant's shape over a cause with a reason of its own: the level the `Display`
/// already printed is skipped, and the reason below it still has to arrive.
#[test]
fn a_level_skipped_as_already_printed_does_not_end_the_walk() {
    let error = layer(
        "IO error: disk full",
        Some(layer("disk full", Some(layer("No space left on device (os error 28)", None)))),
    );

    assert_eq!(describe(&error), "IO error: disk full: No space left on device (os error 28)");
}

#[test]
fn an_empty_cause_leaves_no_trailing_separator() {
    let error = layer("Failed to spawn the worker", Some(layer("", None)));

    assert_eq!(describe(&error), "Failed to spawn the worker");
}

/// The skip is for a cause the `Display` interpolated, not for one whose words happen to end the
/// context sentence, which a bare suffix match dropped. Repeating a word is the cheap failure;
/// losing the reason is the one this function exists to prevent.
#[test]
fn a_cause_that_only_ends_the_context_sentence_is_still_appended() {
    let error = layer("Failed to read the tag", Some(layer("tag", None)));

    assert_eq!(describe(&error), "Failed to read the tag: tag");
}

/// A wrapper that prints exactly its cause, as `#[error(transparent)]` over a source would.
#[test]
fn a_wrapper_that_prints_exactly_its_cause_is_not_repeated() {
    let error = layer("permission denied", Some(layer("permission denied", None)));

    assert_eq!(describe(&error), "permission denied");
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

/// The four struct variants exist so a context message and a typed cause both survive, and
/// `describe` is what puts them back together. Only `network` had a case for it, so
/// `AppError::scanner` and `AppError::watcher` had never been built with a source at all: a
/// constructor that dropped one satisfies every other test in this file, and the failure it
/// hides is a permissions error and a full disk reading identically in a bug report.
///
/// Expectations written out rather than composed from the parts, so the table cannot restate
/// `describe`'s own append rule back at it.
#[test]
fn every_io_boundary_variant_keeps_its_context_and_its_cause() {
    let cause = || std::io::Error::other("no space left on device");

    let cases: [(AppError, &str); 4] = [
        (
            AppError::metadata("Failed to read tags", cause()),
            "Metadata error: Failed to read tags: no space left on device",
        ),
        (
            AppError::scanner("Failed to join the scan pool", cause()),
            "Scanner error: Failed to join the scan pool: no space left on device",
        ),
        (
            AppError::watcher("Failed to watch the folder", cause()),
            "Watcher error: Failed to watch the folder: no space left on device",
        ),
        (
            AppError::network("Failed to reach the directory", cause()),
            "Network error: Failed to reach the directory: no space left on device",
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(
            describe(&error),
            expected,
            "a constructor that drops its source reports the operation and never the reason"
        );
    }
}

/// An error that prints exactly its message, so a chain can be built to any depth and in either
/// `Display` shape without a third-party type deciding the text.
#[derive(Debug)]
struct Layer {
    message: &'static str,
    cause: Option<Box<Layer>>,
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for Layer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

fn layer(message: &'static str, cause: Option<Layer>) -> Layer {
    Layer { message, cause: cause.map(Box::new) }
}
