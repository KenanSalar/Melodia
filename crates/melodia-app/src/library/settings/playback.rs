//! Setters for the Playback and Output cards, plus the persisted playback speed.

use crate::library::playback::FOLLOW_RATE_SUPPORTED;
use crate::services;
use crate::services::settings::{PausedDevice, SettingsData};
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_engine::player::engine::backend::OutputChoice;
use melodia_engine::player::engine::state::{MAX_SPEED, MAX_VOLUME, MIN_SPEED};
use melodia_playback::player::playback::output::convert::Resampler;

pub fn set_gapless_playback(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.playback.gapless_playback = on;
    })
}

pub fn set_output_follow_rate(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.output.output_follow_rate = on;
    })
}

pub fn set_output_resampler(paths: &Paths, resampler: Resampler) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.output.output_resampler = resampler;
    })
}

pub fn set_output_paused_device(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.output.output_paused_device = PausedDevice::from_toggle(on);
    })
}

/// The silence a reopen onto a new rate writes first.
pub fn set_output_resync_ms(paths: &Paths, ms: u32) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.output.output_resync_ms = ms;
    })
}

/// The whole choice, since each of the Output card's pickers changes one part of it.
pub fn set_output_choice(paths: &Paths, choice: &OutputChoice) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, |settings| {
        settings.output.set_output_choice(choice);
    })
}

/// Persist what `library::playback::player_make_bit_perfect` just applied, the `volume` it left
/// included, in one write so a failure can't leave half of it on disk.
pub fn reset_for_bit_perfect(paths: &Paths, volume: u32) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, |settings| {
        apply_bit_perfect_reset(settings, volume);
    })
}

/// Persist Make Bit-Perfect's switch to exclusive output: the `choice` the engine was handed and
/// the reset beside it, in the one write [`reset_for_bit_perfect`] takes for the same reason.
pub fn switch_to_bit_perfect(
    paths: &Paths,
    choice: &OutputChoice,
    volume: u32,
) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, |settings| {
        settings.output.set_output_choice(choice);
        apply_bit_perfect_reset(settings, volume);
    })
}

fn apply_bit_perfect_reset(settings: &mut SettingsData, volume: u32) {
    settings.equalizer.eq_enabled = false;
    settings.replaygain.rg_enabled = false;
    settings.playback.playback_speed = 1.0;
    settings.volume = volume.min(MAX_VOLUME);
    settings.playback.is_muted = false;
    if FOLLOW_RATE_SUPPORTED {
        settings.output.output_follow_rate = true;
    }
}

/// A string token, so a variant can be added without a migration. Anything unknown, a retired
/// token included, falls back to `"none"`, so a bad write can't pin the chip to an unrenderable
/// index.
pub fn set_play_button_animation(paths: &Paths, mode: String) -> Result<(), AppError> {
    let token = match mode.as_str() {
        "none" | "equalizer" => mode,
        _ => "none".to_owned(),
    };
    services::settings::mutate_settings(paths, move |settings| {
        settings.play_button_animation = token;
    })
}

/// Read once, by the next launch's restore.
pub fn set_resume_on_startup(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.playback.resume_on_startup = on;
    })
}

/// Clamped to `MIN_SPEED..=MAX_SPEED`, so the boot restore never has to.
///
/// NaN is refused rather than clamped: `f64::clamp` passes it through, `serde_json` writes it as
/// `null`, and the next launch fails to parse `settings.json` and falls back to defaults, one bad
/// write costing every setting. `clamp_preamp` and `clamp_rg_preamp` carry the same arm.
pub fn set_playback_speed(paths: &Paths, speed: f64) -> Result<(), AppError> {
    if speed.is_nan() {
        return Err(AppError::validation_msg("Playback speed is not a number"));
    }
    let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    services::settings::mutate_settings(paths, move |settings| {
        settings.playback.playback_speed = speed;
    })
}

#[cfg(test)]
#[path = "tests/playback_tests.rs"]
mod tests;
