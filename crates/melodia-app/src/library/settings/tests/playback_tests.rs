//! The playback setters that decide something before the write.
//!
//! The first two guard the same thing from opposite directions: a value the UI should never send,
//! landing in `settings.json` where the next launch has to make sense of it. The bit-perfect reset
//! decides by platform what it may turn on, and has to turn stages off without forgetting how
//! they were set, and the switch to exclusive output has to land with it in one write. The rest
//! of the module is a field assignment whose round trip `services/tests/settings_tests.rs` already
//! covers.

use std::time::Duration;

use crate::services;
use crate::services::settings::SettingsData;
use crate::state::fixtures::{seeded_root, seeded_root_with};
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_engine::player::engine::backend::OutputChoice;
use melodia_engine::player::engine::state::{MAX_SPEED, MAX_VOLUME, MIN_SPEED};
use melodia_playback::player::playback::output::{
    Drive, ExclusiveTuning, OutputMode, RateFallback,
};

use super::{
    set_playback_speed, write_bit_perfect_reset, write_bit_perfect_switch,
    write_play_button_animation,
};

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

    set_playback_speed(&paths, MIN_SPEED / 2.0)?;

    assert!((stored_speed(&paths)? - MIN_SPEED).abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn a_speed_above_the_ceiling_is_lowered_to_it() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    set_playback_speed(&paths, MAX_SPEED * 2.0)?;

    assert!((stored_speed(&paths)? - MAX_SPEED).abs() < f64::EPSILON);
    Ok(())
}

/// The expensive one. A NaN survives `clamp`, serialises as `null`, and takes the whole file down
/// with it on the next launch — every setting the user has, reset, with only a log line to say so.
#[test]
fn a_speed_that_is_not_a_number_is_refused_and_never_written() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;
    let before = stored_speed(&paths)?;

    let refused = set_playback_speed(&paths, f64::NAN);

    assert!(matches!(refused, Err(AppError::Validation(_))));
    assert!((stored_speed(&paths)? - before).abs() < f64::EPSILON);
    Ok(())
}

/// Windows shared mode converts every stream to the mix format, so the row is hidden there. A
/// reset that switched it on would reopen the device at every rate change for nothing, behind a
/// setting nobody can see to turn back off.
#[cfg(target_os = "windows")]
#[test]
fn the_bit_perfect_reset_leaves_follow_rate_off_on_windows() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root()?;

    write_bit_perfect_reset(&paths, MAX_VOLUME)?;

    let settings = services::settings::read_settings(&paths)?;
    assert!(!settings.output.output_follow_rate, "the reset turned on a row Windows hides");
    Ok(())
}

/// Everywhere else the system resamples a stream to the rate it runs at, so following the file's
/// rate is part of what the reset calls bit-perfect: the one row it turns on rather than off.
#[cfg(not(target_os = "windows"))]
#[test]
fn the_bit_perfect_reset_follows_the_files_rate_where_the_system_resamples() -> Result<(), AppError>
{
    let (_tmp, paths) = seeded_root()?;

    write_bit_perfect_reset(&paths, MAX_VOLUME)?;

    let settings = services::settings::read_settings(&paths)?;
    assert!(settings.output.output_follow_rate, "the reset left the system resampling the file");
    Ok(())
}

/// Every stage the live reset cleared is written in the one go, or the next launch brings back
/// one the panel said it had cleared.
#[test]
fn the_bit_perfect_reset_writes_every_stage_it_cleared() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|settings| {
        settings.equalizer.eq_enabled = true;
        settings.replaygain.rg_enabled = true;
        settings.playback.playback_speed = 1.5;
        settings.playback.is_muted = true;
        settings.volume = 30;
    })?;

    write_bit_perfect_reset(&paths, MAX_VOLUME)?;

    let s = services::settings::read_settings(&paths)?;
    let written = (
        s.equalizer.eq_enabled,
        s.replaygain.rg_enabled,
        s.playback.playback_speed.to_bits(),
        s.playback.is_muted,
        s.volume,
    );
    assert_eq!(
        written,
        (false, false, 1.0_f64.to_bits(), false, MAX_VOLUME),
        "(equalizer, ReplayGain, speed bits, muted, volume)"
    );
    Ok(())
}

/// The reset switches a stage off and nothing more, so turning it back on finds it as the user left
/// it. A flattened curve or a zeroed preamp would be a second reset nobody asked for.
#[test]
fn the_bit_perfect_reset_keeps_how_each_stage_it_cleared_was_set() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|settings| {
        settings.equalizer.eq_enabled = true;
        settings.equalizer.eq_band_gains.fill(6.0);
        settings.equalizer.eq_preamp = -4.0;
        settings.replaygain.rg_enabled = true;
        settings.replaygain.rg_preamp = 3.0;
    })?;
    let tuning = |s: &SettingsData| {
        (
            s.equalizer.eq_band_gains.clone(),
            s.equalizer.eq_preamp.to_bits(),
            s.equalizer.eq_selected_preset.clone(),
            s.replaygain.rg_preamp.to_bits(),
            s.replaygain.rg_mode.clone(),
        )
    };
    let before = tuning(&services::settings::read_settings(&paths)?);

    write_bit_perfect_reset(&paths, MAX_VOLUME)?;

    let after = tuning(&services::settings::read_settings(&paths)?);
    assert_eq!(after, before, "(curve, preamp bits, preset, ReplayGain preamp bits, mode)");
    Ok(())
}

/// Make Bit-Perfect from shared output once took two writes, so a refused second one left
/// exclusive output on disk with EQ and `ReplayGain` still on.
#[test]
fn the_switch_to_bit_perfect_writes_the_claim_and_the_reset_together() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|settings| {
        settings.equalizer.eq_enabled = true;
        settings.replaygain.rg_enabled = true;
    })?;
    let choice = OutputChoice {
        mode: OutputMode::Exclusive,
        device: Some("hw:CARD=DAC,DEV=0".to_owned()),
        tuning: ExclusiveTuning::new(Duration::from_millis(40), Drive::Polling),
        hardware_volume: true,
        rate_fallback: RateFallback::Resample,
    };

    write_bit_perfect_switch(&paths, &choice, MAX_VOLUME)?;

    let s = services::settings::read_settings(&paths)?;
    let written = (s.output.output_choice(), s.equalizer.eq_enabled, s.replaygain.rg_enabled);
    assert_eq!(written, (choice, false, false), "(choice, equalizer, ReplayGain)");
    Ok(())
}
