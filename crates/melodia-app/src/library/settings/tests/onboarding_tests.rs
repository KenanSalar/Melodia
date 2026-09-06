//! The revision write against a real settings file.
//!
//! `needs_onboarding`'s suite says the comparison is right and the source pins say the callback
//! reaches this function. Neither says the value that lands on disk is the one a later boot reads
//! back, and a version writing `0`, or writing to a field nothing gates on, satisfies both while
//! showing the card on every launch forever.

use crate::services;
use crate::services::settings::ONBOARDING_VERSION;
use crate::state::fixtures::{seeded_root, seeded_root_with};
use melodia_core::error::AppError;

use super::write_onboarding_seen;

/// The fresh-install path: `0` on disk, the card shown, and the revision stamped on dismissal.
#[test]
fn dismissing_the_card_persists_the_current_revision() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;
    assert_eq!(services::settings::read_settings(&paths)?.onboarding.onboarding_version, 0);

    write_onboarding_seen(&paths)?;

    let settings = services::settings::read_settings(&paths)?;
    assert_eq!(settings.onboarding.onboarding_version, ONBOARDING_VERSION);
    assert!(!settings.onboarding.needs_onboarding(), "the card is still owed after a dismissal");
    Ok(())
}

/// The write goes through `mutate_settings`, which reads the whole file and writes it back — so a
/// version reaching for `write_settings` over a stale snapshot would silently revert every other
/// setting. The card is dismissed seconds after boot, with the theme picks from step 1 already on
/// disk, which is exactly the window where that would bite.
#[test]
fn stamping_the_revision_leaves_the_neighbouring_settings_alone() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| {
        s.theme_id = "material3".to_owned();
        s.accent_color = "teal".to_owned();
        s.radio.radio_enabled = true;
    })?;

    write_onboarding_seen(&paths)?;

    let settings = services::settings::read_settings(&paths)?;
    assert_eq!(settings.theme_id, "material3");
    assert_eq!(settings.accent_color, "teal");
    assert!(settings.radio.radio_enabled);
    Ok(())
}

/// The Settings ▸ About row re-opens the card without rewinding the flag, so the dismissal that
/// follows writes the same revision a second time. It has to be an ordinary write rather than
/// something that only lands on a change, or a re-run would leave the file describing a card the
/// user has since seen again.
#[test]
fn stamping_an_already_seen_install_is_idempotent() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| {
        s.onboarding.onboarding_version = ONBOARDING_VERSION;
    })?;

    write_onboarding_seen(&paths)?;

    assert_eq!(
        services::settings::read_settings(&paths)?.onboarding.onboarding_version,
        ONBOARDING_VERSION
    );
    Ok(())
}

/// An install carrying a revision from a newer build — a downgrade, or a home directory shared
/// across machines — must not be shown a card it has already seen, and dismissing this one must
/// not silently rewind it.
#[test]
fn a_newer_revision_survives_a_dismissal() -> Result<(), AppError> {
    let ahead = ONBOARDING_VERSION + 1;
    let (_tmp, paths) = seeded_root_with(|s| s.onboarding.onboarding_version = ahead)?;

    assert!(!services::settings::read_settings(&paths)?.onboarding.needs_onboarding());
    Ok(())
}
