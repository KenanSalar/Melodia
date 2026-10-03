//! Setters for the Playback tab's Playback and Output cards, plus the persisted
//! playback speed. Each only commits the disk write: the matching UI callback
//! applies the runtime side, synchronously for a toggle and on the blocking pool
//! where it has to wait on the output device.

use crate::library::playback::FOLLOW_RATE_SUPPORTED;
use crate::services;
use crate::services::settings::{PausedDevice, SettingsData};
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_engine::player::engine::backend::OutputChoice;
use melodia_engine::player::engine::state::{MAX_SPEED, MAX_VOLUME, MIN_SPEED};

/// Persist the user toggle for "Gapless Playback". The runtime effect
/// (gating `preload_gapless` inside the 500 ms position monitor in
/// `crates/melodia-engine/src/player/engine/handlers.rs`) is applied
/// synchronously by the UI callback through
/// `library::playback::player_set_gapless` *before* this async disk write,
/// so the next staging tick picks up the new value even when the file
/// rewrite hasn't completed.
pub fn set_gapless_playback(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.playback.gapless_playback = on;
    })
}

/// Persist "Match the File's Sample Rate". The engine already holds the new value through
/// `library::playback::player_set_follow_rate`, and acts on it at the next track.
pub fn set_output_follow_rate(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.output.output_follow_rate = on;
    })
}

/// Persist "Give the Device Back When Paused". The engine already holds the new value through
/// `library::playback::player_set_release_when_paused`, and the monitor reads it on its next tick.
pub fn set_output_paused_device(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.output.output_paused_device = PausedDevice::from_toggle(on);
    })
}

/// Persist the silence a reopen onto a new rate writes first, which
/// `library::playback::player_set_resync_hold` already handed the engine.
pub fn set_output_resync_ms(state: &AppState, ms: u32) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.output.output_resync_ms = ms;
    })
}

/// Persist the whole output choice the engine was just handed, since each of the Output card's
/// pickers changes one part of it.
pub fn set_output_choice(state: &AppState, choice: &OutputChoice) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, |settings| {
        settings.output.set_output_choice(choice);
    })
}

/// Persist what `library::playback::player_make_bit_perfect` just applied, the `volume` it left
/// included, in one write so a failure can't leave half of it on disk.
pub fn reset_for_bit_perfect(state: &AppState, volume: u32) -> Result<(), AppError> {
    write_bit_perfect_reset(&state.paths, volume)
}

/// Persist Make Bit-Perfect's switch to exclusive output: the `choice` the engine was handed and
/// the reset beside it, in the one write [`reset_for_bit_perfect`] takes for the same reason.
pub fn switch_to_bit_perfect(
    state: &AppState,
    choice: &OutputChoice,
    volume: u32,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, |settings| {
        settings.output.set_output_choice(choice);
        apply_bit_perfect_reset(settings, volume);
    })
}

/// [`reset_for_bit_perfect`]'s body, narrowed so what it leaves alone can be read back.
fn write_bit_perfect_reset(paths: &Paths, volume: u32) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, |settings| {
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

/// Persist the user's "Play Button Animation" pick (None / Equalizer).
/// The on-disk value stays a string token so future variants can be
/// added without a migration; anything outside the known set (including
/// the retired `"ripple"` token) falls back to `"none"` so a malformed
/// UI write (or a hand-edited `settings.json` with a typo) can't pin the
/// chip to an unrenderable index. Runtime effect (the `PlayButton`
/// switching overlays) is reactive off the Slint
/// `Settings.play-button-animation-idx` property, so the UI already
/// repainted before this disk write is scheduled.
pub fn set_play_button_animation(state: &AppState, mode: String) -> Result<(), AppError> {
    write_play_button_animation(&state.paths, mode)
}

/// [`set_play_button_animation`]'s body, narrowed so the fallback can be driven.
fn write_play_button_animation(paths: &Paths, mode: String) -> Result<(), AppError> {
    let token = match mode.as_str() {
        "none" | "equalizer" => mode,
        _ => "none".to_owned(),
    };
    services::settings::mutate_settings(paths, move |settings| {
        settings.play_button_animation = token;
    })
}

/// Persist the user toggle for "Resume on Startup". No runtime side
/// effect at toggle time — the flag is consulted once, at the next
/// `main.rs` startup after `restore_persisted_playback`, so a single-phase
/// disk write is all that's needed. The on-disk default is `false`
/// (`PlaybackFlags::default()` in `services/settings/playback.rs`), so
/// first-launch users land with auto-resume off.
pub fn set_resume_on_startup(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.playback.resume_on_startup = on;
    })
}

/// Persist the playback speed so it survives restarts, as repeat, shuffle and volume do. Clamped to
/// the player's `MIN_SPEED..=MAX_SPEED` here too, so a malformed write can't pin an out-of-range
/// value the boot restore would then have to clamp. Takes the paths rather than the state because
/// its caller, `library::playback::player_set_playback_speed_committed`, holds a `PlaybackContext`.
///
/// NaN is refused rather than clamped, unlike the two bounds either side of it: `f64::clamp`
/// passes it through, `serde_json` writes a non-finite float as `null`, and the next launch fails
/// to parse `settings.json` and falls back to defaults — one malformed write costing the user
/// every setting they have. `clamp_preamp` and `clamp_rg_preamp` each carry the same arm.
pub fn set_playback_speed(paths: &Paths, speed: f64) -> Result<(), AppError> {
    if speed.is_nan() {
        return Err(AppError::Validation("Playback speed is not a number".to_owned()));
    }
    let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    services::settings::mutate_settings(paths, move |settings| {
        settings.playback.playback_speed = speed;
    })
}

#[cfg(test)]
#[path = "tests/playback_tests.rs"]
mod tests;
