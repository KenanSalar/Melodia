//! `record_launch` against a real settings file, which is the half `count_launch`'s suite and the
//! source-text pin between them cannot reach.
//!
//! One says the arithmetic is right, the other that the guard is written above the mutate. Neither
//! says the counter being counted is the persisted one, and a version that read a fresh default
//! every launch would satisfy both while never reaching the threshold at all.

use crate::services;
use crate::state::fixtures::seeded_root_with;
use melodia_core::error::AppError;

use super::{PROMPT_AT_LAUNCH, record_launch_at};

/// The launch that asks, reached from a count an earlier launch left on disk.
#[test]
fn the_launch_on_the_threshold_asks_from_a_persisted_count() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| s.support.launch_count = PROMPT_AT_LAUNCH - 1)?;

    let due = record_launch_at(&paths)?;

    assert!(due, "the count came off the file, not a default");
    assert_eq!(services::settings::read_settings(&paths)?.support.launch_count, PROMPT_AT_LAUNCH);
    Ok(())
}

#[test]
fn the_launch_below_it_does_not_ask() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| s.support.launch_count = PROMPT_AT_LAUNCH - 2)?;

    let due = record_launch_at(&paths)?;

    assert!(!due);
    Ok(())
}

/// Once the prompt is spent the counter stops moving, which is what stops a settled install
/// rewriting `settings.json` on every boot to store a number nothing reads.
#[test]
fn a_settled_install_neither_asks_nor_counts() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| {
        s.support.launch_count = PROMPT_AT_LAUNCH;
        s.support.support_prompt_seen = true;
    })?;

    let due = record_launch_at(&paths)?;

    assert!(!due);
    assert_eq!(services::settings::read_settings(&paths)?.support.launch_count, PROMPT_AT_LAUNCH);
    Ok(())
}
