//! `last_static_accent`, which the two writers treat differently on purpose.
//!
//! It is the accent the UI falls back to when Color Style is switched off or a cover yields no
//! palette, so a Material You pick must never overwrite it. The migration exists because losing it
//! is silent: the user swaps theme once and their custom accent is simply gone, with nothing in
//! the file to say it was ever there.

use crate::services;
use crate::state::fixtures::{seeded_root, seeded_root_with};
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_core::themes::MATERIAL_YOU_ACCENT_ID;

use super::{ThemePreference, seed_preference, write_appearance};

const THEME: &str = "catppuccin";
const VARIANT: &str = "mocha";
const STATIC_ACCENT: &str = "yellow";

/// `THEME`'s entry, which every case below expects to find — an absent one is a failure with its
/// own message rather than a `None` folded into the assertion.
fn stored_preference(paths: &Paths) -> Result<ThemePreference, AppError> {
    services::settings::read_settings(paths)?
        .theme_preferences
        .remove(THEME)
        .ok_or_else(|| AppError::Validation(format!("no preference stored for {THEME}")))
}

/// A root seeded for `THEME`, with `accent` as the accent an earlier build left at the top level.
fn rooted_at_accent(accent: &str) -> Result<(tempfile::TempDir, Paths), AppError> {
    let accent = accent.to_owned();
    seeded_root_with(move |s| {
        s.theme_id = THEME.to_owned();
        s.theme_variant = VARIANT.to_owned();
        s.accent_color = accent;
    })
}

/// The migration runs once and writes the whole file, so the guard is what stops a settled install
/// rewriting `settings.json` on every launch — and, worse, writing back whatever snapshot the
/// caller happened to be holding.
#[test]
fn seeding_over_an_entry_that_exists_writes_nothing() -> Result<(), AppError> {
    let (_tmp, paths) = rooted_at_accent(STATIC_ACCENT)?;
    let mut settings = services::settings::read_settings(&paths)?;
    settings.theme_preferences.insert(
        THEME.to_owned(),
        ThemePreference {
            variant: VARIANT.to_owned(),
            accent: STATIC_ACCENT.to_owned(),
            last_static_accent: Some(STATIC_ACCENT.to_owned()),
        },
    );
    settings.accent_color = "mauve".to_owned();

    seed_preference(&paths, settings)?;

    let on_disk = services::settings::read_settings(&paths)?;
    assert_eq!(on_disk.accent_color, STATIC_ACCENT, "the snapshot never reached the file");
    assert!(on_disk.theme_preferences.is_empty(), "and neither did its entry");
    Ok(())
}

#[test]
fn seeding_records_a_static_accent_as_the_one_to_fall_back_to() -> Result<(), AppError> {
    let (_tmp, paths) = rooted_at_accent(STATIC_ACCENT)?;
    let settings = services::settings::read_settings(&paths)?;

    seed_preference(&paths, settings)?;

    assert_eq!(stored_preference(&paths)?.last_static_accent, Some(STATIC_ACCENT.to_owned()));
    Ok(())
}

/// A file whose accent was already Material You has no static accent to remember, and storing the
/// sentinel as one would hand the fallback a value that is not a colour.
#[test]
fn seeding_a_material_you_accent_records_no_fallback() -> Result<(), AppError> {
    let (_tmp, paths) = rooted_at_accent(MATERIAL_YOU_ACCENT_ID)?;
    let settings = services::settings::read_settings(&paths)?;

    seed_preference(&paths, settings)?;

    assert_eq!(stored_preference(&paths)?.last_static_accent, None);
    Ok(())
}

/// The one the migration exists to protect. Turning Color Style on has to leave the accent the
/// user chose where the fallback can find it.
#[test]
fn picking_material_you_preserves_the_static_accent_underneath() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;
    write_appearance(&paths, THEME.to_owned(), VARIANT.to_owned(), STATIC_ACCENT.to_owned())?;

    write_appearance(
        &paths,
        THEME.to_owned(),
        VARIANT.to_owned(),
        MATERIAL_YOU_ACCENT_ID.to_owned(),
    )?;

    assert_eq!(stored_preference(&paths)?.last_static_accent, Some(STATIC_ACCENT.to_owned()));
    Ok(())
}

#[test]
fn picking_material_you_for_a_theme_with_no_history_records_no_fallback() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_appearance(
        &paths,
        THEME.to_owned(),
        VARIANT.to_owned(),
        MATERIAL_YOU_ACCENT_ID.to_owned(),
    )?;

    assert_eq!(stored_preference(&paths)?.last_static_accent, None);
    Ok(())
}

#[test]
fn picking_a_static_accent_makes_it_the_one_to_fall_back_to() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;
    write_appearance(
        &paths,
        THEME.to_owned(),
        VARIANT.to_owned(),
        MATERIAL_YOU_ACCENT_ID.to_owned(),
    )?;

    write_appearance(&paths, THEME.to_owned(), VARIANT.to_owned(), STATIC_ACCENT.to_owned())?;

    assert_eq!(stored_preference(&paths)?.last_static_accent, Some(STATIC_ACCENT.to_owned()));
    Ok(())
}

/// The per-theme entry and the three top-level fields are written in one pass and have to agree:
/// the entry is what a later switch back reads, the top-level fields are what this boot paints
/// from.
#[test]
fn a_pick_lands_in_both_the_entry_and_the_top_level_fields() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_appearance(&paths, THEME.to_owned(), VARIANT.to_owned(), STATIC_ACCENT.to_owned())?;

    let settings = services::settings::read_settings(&paths)?;
    assert_eq!(settings.theme_id, THEME);
    assert_eq!(settings.theme_variant, VARIANT);
    assert_eq!(settings.accent_color, STATIC_ACCENT);
    let entry =
        settings.theme_preferences.get(THEME).map(|p| (p.variant.clone(), p.accent.clone()));
    assert_eq!(entry, Some((VARIANT.to_owned(), STATIC_ACCENT.to_owned())));
    Ok(())
}
