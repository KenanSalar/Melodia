//! The two playback setters that decide something before the write.
//!
//! Both guard the same thing from opposite directions: a value the UI should never send, landing
//! in `settings.json` where the next launch has to make sense of it. The rest of the module is a
//! field assignment whose round trip `services/tests/settings_tests.rs` already covers.

use crate::services;
use crate::state::fixtures::seeded_root;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_engine::player::engine::state::{MAX_SPEED, MIN_SPEED};

use super::{write_play_button_animation, write_playback_speed};

fn stored_token(paths: &Paths) -> Result<String, AppError> {
    Ok(services::settings::read_settings(paths)?.play_button_animation)
}

fn stored_speed(paths: &Paths) -> Result<f64, AppError> {
    Ok(services::settings::read_settings(paths)?.playback.playback_speed)
}

#[test]
fn a_known_animation_token_is_stored_as_it_came() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_play_button_animation(&paths, "equalizer".to_owned())?;

    assert_eq!(stored_token(&paths)?, "equalizer");
    Ok(())
}

/// `"ripple"` was a real token an older build wrote, so it is on disk in installs that predate its
/// removal. Without the fallback it survives the read and indexes a chip that no longer exists.
#[test]
fn the_retired_ripple_token_falls_back_to_none() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_play_button_animation(&paths, "ripple".to_owned())?;

    assert_eq!(stored_token(&paths)?, "none");
    Ok(())
}

#[test]
fn an_unknown_animation_token_falls_back_to_none() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_play_button_animation(&paths, "sparkle".to_owned())?;

    assert_eq!(stored_token(&paths)?, "none");
    Ok(())
}

/// Both bounds, worked against the real constants rather than round numbers.
#[test]
fn a_speed_below_the_floor_is_raised_to_it() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_playback_speed(&paths, MIN_SPEED / 2.0)?;

    assert!((stored_speed(&paths)? - MIN_SPEED).abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn a_speed_above_the_ceiling_is_lowered_to_it() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_playback_speed(&paths, MAX_SPEED * 2.0)?;

    assert!((stored_speed(&paths)? - MAX_SPEED).abs() < f64::EPSILON);
    Ok(())
}

/// The expensive one. A NaN survives `clamp`, serialises as `null`, and takes the whole file down
/// with it on the next launch — every setting the user has, reset, with only a log line to say so.
#[test]
fn a_speed_that_is_not_a_number_is_refused_and_never_written() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;
    let before = stored_speed(&paths)?;

    let refused = write_playback_speed(&paths, f64::NAN);

    assert!(matches!(refused, Err(AppError::Validation(_))));
    assert!((stored_speed(&paths)? - before).abs() < f64::EPSILON);
    Ok(())
}
