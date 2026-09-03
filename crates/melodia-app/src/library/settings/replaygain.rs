//! `ReplayGain`-section setters. Like the equalizer setters, the runtime effect
//! (applying to the live playback engine) is done synchronously by the UI callback
//! through `library::playback::player_set_replaygain_*` *before* these async disk
//! writes, so the sound changes immediately and the `settings.json` commit only
//! persists the choice. Mirrors the shape of [`super::equalizer`].

use crate::services;
use crate::state::AppState;
use melodia_core::error::AppError;
use melodia_playback::player::playback::replaygain::{self, RgMode};

/// Persist the `ReplayGain` on/off toggle. Defaults to `false` on first launch
/// (`ReplayGainFlags::default()`), so new installs land with `ReplayGain` inert.
pub fn set_replaygain_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.replaygain.rg_enabled = enabled;
    })
}

/// Persist the `ReplayGain` mode (Track / Album). Stored as a lowercase token so a
/// hand-edited `settings.json` stays human-readable; hydration falls back to
/// Album on an unknown value.
pub fn set_replaygain_mode(state: &AppState, mode: RgMode) -> Result<(), AppError> {
    let token = mode.as_settings_str().to_owned();
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.replaygain.rg_mode = token;
    })
}

/// Persist the `ReplayGain` preamp (dB). Clamped here too so a hand-edited
/// `settings.json` can't pin an out-of-range value. The runtime effect is applied
/// synchronously by the UI callback through
/// `library::playback::player_set_replaygain_preamp` before this disk write.
pub fn set_replaygain_preamp(state: &AppState, preamp_db: f32) -> Result<(), AppError> {
    let preamp_db = replaygain::clamp_rg_preamp(preamp_db);
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.replaygain.rg_preamp = preamp_db;
    })
}

/// Persist the static peak-based clip-guard toggle. Defaults **on** so a boosted
/// track never clips once `ReplayGain` is enabled.
pub fn set_replaygain_prevent_clipping(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.replaygain.rg_prevent_clipping = on;
    })
}
