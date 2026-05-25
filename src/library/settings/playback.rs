//! Playback-section setters (gapless playback, play-button animation,
//! resume-on-startup). Runtime side effects are applied synchronously by
//! the matching UI callbacks; these helpers only commit the disk write.

use crate::error::AppError;
use crate::services;
use crate::state::AppState;

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
pub fn set_play_button_animation(
    state: &AppState,
    mode: String,
) -> Result<(), AppError> {
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
/// `main.rs` startup after `restore_persisted_queue`, so a single-phase
/// disk write is all that's needed. The on-disk default is `false`
/// (`PlaybackFlags::default()` in `src/services/settings.rs`), so
/// first-launch users land with auto-resume off.
pub fn set_resume_on_startup(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.playback.resume_on_startup = on;
    })
}
