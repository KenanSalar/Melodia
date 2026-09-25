//! Playback-section setters (gapless playback, play-button animation,
//! resume-on-startup). Runtime side effects are applied synchronously by
//! the matching UI callbacks; these helpers only commit the disk write.

use crate::library::playback::FOLLOW_RATE_SUPPORTED;
use crate::services;
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
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

/// Persist what `library::playback::player_make_bit_perfect` just applied, in one write so a
/// failure can't leave half of it on disk.
pub fn reset_for_bit_perfect(state: &AppState) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, |settings| {
        settings.equalizer.eq_enabled = false;
        settings.replaygain.rg_enabled = false;
        settings.playback.playback_speed = 1.0;
        settings.volume = MAX_VOLUME;
        settings.playback.is_muted = false;
        if FOLLOW_RATE_SUPPORTED {
            settings.output.output_follow_rate = true;
        }
    })
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

/// Persist the user's chosen playback speed so it survives restarts
/// (mirrors how repeat / shuffle / volume persist). The runtime effect
/// (applying the multiplier to the live playback engine) is done synchronously
/// by the UI callback through `library::playback::player_set_playback_speed`
/// *before* this disk write is scheduled. Clamped to the player's
/// `MIN_SPEED..=MAX_SPEED` range here too, so a malformed UI write can't
/// pin an out-of-range value the boot restore would then have to clamp.
pub fn set_playback_speed(state: &AppState, speed: f64) -> Result<(), AppError> {
    write_playback_speed(&state.paths, speed)
}

/// [`set_playback_speed`]'s body, narrowed so both bounds and the refusal can be driven.
///
/// NaN is refused rather than clamped, unlike the two bounds either side of it: `f64::clamp`
/// passes it through, `serde_json` writes a non-finite float as `null`, and the next launch fails
/// to parse `settings.json` and falls back to defaults — one malformed write costing the user
/// every setting they have. `clamp_preamp` and `clamp_rg_preamp` each carry the same arm.
fn write_playback_speed(paths: &Paths, speed: f64) -> Result<(), AppError> {
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
