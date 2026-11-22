//! Each switch writes its own field.
//!
//! Four functions that differ only in which field of one struct they assign, over a settings type
//! whose fields don't share a default. A copy that landed the wrong one persists a switch the user
//! never touched and reads as the toggle they did touch not sticking.

use super::{
    set_lyrics_auto_lookup, set_lyrics_enabled, set_lyrics_online_enabled,
    set_lyrics_romanization_shown,
};
use crate::services;
use crate::state::fixtures::seeded_root;
use melodia_core::error::AppError;

/// The four switches as they sit on disk, in declaration order.
fn stored(paths: &melodia_core::config::Paths) -> Result<(bool, bool, bool, bool), AppError> {
    let lyrics = services::settings::read_settings(paths)?.lyrics;
    Ok((
        lyrics.lyrics_enabled,
        lyrics.lyrics_online_enabled,
        lyrics.lyrics_auto_lookup,
        lyrics.lyrics_romanization_shown,
    ))
}

#[test]
fn enabling_lyrics_leaves_the_other_three_alone() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    set_lyrics_enabled(&paths, true)?;

    assert_eq!(stored(&paths)?, (true, false, true, true));
    Ok(())
}

#[test]
fn enabling_the_lookup_leaves_the_other_three_alone() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    set_lyrics_online_enabled(&paths, true)?;

    assert_eq!(stored(&paths)?, (false, true, true, true));
    Ok(())
}

#[test]
fn turning_the_automatic_lookup_off_leaves_the_other_three_alone() -> Result<(), AppError> {
    // Ships on, so a wrong field assignment is a switch turned off by accident while appearing to
    // have worked.
    let (_tmp, paths) = seeded_root()?;

    set_lyrics_auto_lookup(&paths, false)?;

    assert_eq!(stored(&paths)?, (false, false, false, true));
    Ok(())
}

#[test]
fn hiding_the_romanization_leaves_the_other_three_alone() -> Result<(), AppError> {
    // Ships on, for the automatic lookup's reason.
    let (_tmp, paths) = seeded_root()?;

    set_lyrics_romanization_shown(&paths, false)?;

    assert_eq!(stored(&paths)?, (false, false, true, false));
    Ok(())
}

#[test]
fn a_switch_written_twice_keeps_the_second_answer() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    set_lyrics_online_enabled(&paths, true)?;
    set_lyrics_online_enabled(&paths, false)?;

    assert_eq!(stored(&paths)?, (false, false, true, true));
    Ok(())
}
