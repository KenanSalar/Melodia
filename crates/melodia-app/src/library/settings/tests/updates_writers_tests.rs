//! What the skip writer composes through the file.
//!
//! `UpdateFlags`' own suite settles what each method does to the struct. What is left here is the
//! skip's round trip, which is silent when wrong: a skip that fails to clear is an update the user
//! is simply never offered again, with nothing anywhere to say why.

use crate::services;
use crate::state::fixtures::seeded_root;
use melodia_core::error::AppError;

use super::write_skipped_release;

const SKIPPED: &str = "v1.2.3";

/// The skip is spelled as an empty string rather than an absent key, because the notify gate
/// compares it against the live manifest's version — anything else it could be reset to would go
/// on suppressing the toast for whatever version happened to match.
#[test]
fn a_skip_survives_the_file_and_resets_to_the_empty_string() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_skipped_release(&paths, SKIPPED.to_owned())?;
    assert_eq!(services::settings::read_settings(&paths)?.updates.skipped_release, SKIPPED);

    services::settings::mutate_settings(&paths, |settings| {
        settings.updates.forget_skip(SKIPPED);
    })?;

    assert_eq!(services::settings::read_settings(&paths)?.updates.skipped_release, "");
    Ok(())
}
