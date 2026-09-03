//! Playback-section setters (gapless playback, play-button animation,
//! resume-on-startup). Runtime side effects are applied synchronously by
//! the matching UI callbacks; these helpers only commit the disk write.

use crate::services;
use crate::state::AppState;
use melodia_core::error::AppError;

/// Persist the user toggle for "Gapless Playback". The runtime effect
/// (gating `preload_gapless` inside the 500 ms position monitor in
/// `src/player/handlers.rs`) is applied synchronously by the UI callback
/// through `library::playback::player_set_gapless` *before* this async
/// disk write, so the next staging tick picks up the new value even when
/// the file rewrite hasn't completed.
pub fn set_gapless_playback(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.playback.gapless_playback = on;
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
    let token = match mode.as_str() {
        "none" | "equalizer" => mode,
        _ => "none".to_owned(),
    };
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.play_button_animation = token;
    })
}

/// Persist the user toggle for "Resume on Startup". No runtime side
/// effect at toggle time — the flag is consulted once, at the next
/// `main.rs` startup after `restore_persisted_playback`, so a single-phase
/// disk write is all that's needed. The on-disk default is `false`
/// (`PlaybackFlags::default()` in `services/settings/data.rs`), so
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
/// `MIN_SPEED..=MAX_SPEED` range here too so a hand-edited `settings.json`
/// can't pin an out-of-range value.
pub fn set_playback_speed(state: &AppState, speed: f64) -> Result<(), AppError> {
    let speed = speed.clamp(
        melodia_engine::player::engine::state::MIN_SPEED,
        melodia_engine::player::engine::state::MAX_SPEED,
    );
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.playback.playback_speed = speed;
    })
}
