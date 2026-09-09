//! Each switch writes its own field.
//!
//! Three functions that differ only in which field of one struct they assign, over a settings type
//! whose other two fields have the opposite default. A copy that landed the wrong one persists a
//! switch the user never touched and reads as the toggle they did touch not sticking.

use super::{
    write_lyrics_online_enabled, write_lyrics_panel_shown, write_lyrics_romanization_shown,
};
use crate::services;
use crate::state::fixtures::seeded_root;
use melodia_core::error::AppError;

/// The three switches as they sit on disk, in declaration order.
fn stored(paths: &melodia_core::config::Paths) -> Result<(bool, bool, bool), AppError> {
    let lyrics = services::settings::read_settings(paths)?.lyrics;
    Ok((lyrics.lyrics_panel_shown, lyrics.lyrics_online_enabled, lyrics.lyrics_romanization_shown))
}

#[test]
fn showing_the_panel_leaves_the_other_two_alone() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_lyrics_panel_shown(&paths, true)?;

    assert_eq!(stored(&paths)?, (true, false, true));
    Ok(())
}

#[test]
fn enabling_the_lookup_leaves_the_other_two_alone() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_lyrics_online_enabled(&paths, true)?;

    assert_eq!(stored(&paths)?, (false, true, true));
    Ok(())
}

#[test]
fn hiding_the_romanization_leaves_the_other_two_alone() -> Result<(), AppError> {
    // The one switch that ships on, so it is the one a wrong field assignment turns off by
    // accident while appearing to have worked.
    let (_tmp, paths) = seeded_root()?;

    write_lyrics_romanization_shown(&paths, false)?;

    assert_eq!(stored(&paths)?, (false, false, false));
    Ok(())
}

#[test]
fn a_switch_written_twice_keeps_the_second_answer() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_lyrics_online_enabled(&paths, true)?;
    write_lyrics_online_enabled(&paths, false)?;

    assert_eq!(stored(&paths)?, (false, false, true));
    Ok(())
}
