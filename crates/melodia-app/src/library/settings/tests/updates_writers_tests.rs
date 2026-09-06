//! What the updater writers compose through the file.
//!
//! `UpdateFlags`' own suite settles what each method does to the struct. What is left to these
//! four is the conversion in front of one and the pairing between the other two, and both are
//! silent when wrong: a skip that fails to clear is an update the user is simply never offered
//! again, with nothing anywhere to say why.

use chrono::{DateTime, TimeZone, Utc};

use crate::services;
use crate::state::fixtures::seeded_root;
use melodia_core::error::AppError;

use super::{
    clear_skipped_release, write_check_failure, write_check_success, write_skipped_release,
};

const SKIPPED: &str = "v1.2.3";

/// An instant with no meaning beyond being one the test can name back as seconds.
fn at(unix: i64) -> Result<DateTime<Utc>, AppError> {
    Utc.timestamp_opt(unix, 0)
        .single()
        .ok_or_else(|| AppError::Validation("not a representable instant".into()))
}

/// The one thing the doors own on top of the flags: a `DateTime<Utc>` reaches the file as its
/// seconds. Getting the unit wrong re-arms the daily check on a cadence nothing would notice was
/// wrong until it stopped checking for weeks.
#[test]
fn a_success_stores_the_instant_as_seconds_and_clears_the_failure_run() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;
    write_check_failure(&paths, at(1_700_000_000)?)?;

    write_check_success(&paths, at(1_700_000_600)?, Some("v2.0.0".to_owned()), None)?;

    let updates = services::settings::read_settings(&paths)?.updates;
    assert_eq!(updates.last_check_unix, 1_700_000_600);
    assert_eq!(updates.consecutive_failures, 0, "the run the retry cadence reads is over");
    assert_eq!(updates.last_known_release, "v2.0.0");
    Ok(())
}

/// The skip is spelled as an empty string rather than an absent key, because the notify gate
/// compares it against the live manifest's version — anything else it could be reset to would go
/// on suppressing the toast for whatever version happened to match.
#[test]
fn a_skip_survives_the_file_and_resets_to_the_empty_string() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_skipped_release(&paths, SKIPPED.to_owned())?;
    assert_eq!(services::settings::read_settings(&paths)?.updates.skipped_release, SKIPPED);

    clear_skipped_release(&paths)?;

    assert_eq!(services::settings::read_settings(&paths)?.updates.skipped_release, "");
    Ok(())
}
