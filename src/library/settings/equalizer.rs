//! Equalizer-section setters. The runtime effect (applying gains to the live
//! Rodio EQ) is done synchronously by the UI callback through
//! `library::playback::player_set_eq_*` *before* these async disk writes, so the
//! sound changes immediately and the `settings.json` commit only persists the
//! choice. Mirrors the shape of [`super::playback`].

use crate::error::AppError;
use crate::player::equalizer;
use crate::services;
use crate::state::AppState;

/// Persist the EQ on/off toggle. Defaults to `false` on first launch
/// (`EqualizerFlags::default()`), so new installs land with the EQ inert.
pub fn set_eq_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.equalizer.eq_enabled = enabled;
    })
}

/// Persist all band gains. Clamped + length-normalised here too so a
/// hand-edited or wrong-length `settings.json` array can't pin a bad value.
pub fn set_eq_band_gains(state: &AppState, gains: &[f32]) -> Result<(), AppError> {
    let norm = equalizer::normalize_gains(gains).to_vec();
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.equalizer.eq_band_gains = norm;
    })
}

/// Persist the selected preset name (a built-in name or the `"Custom"` sentinel
/// the UI uses for a hand-tuned curve). Stored verbatim; hydration falls back to
/// a Custom display when the name isn't one of the built-in presets.
pub fn set_eq_selected_preset(state: &AppState, preset: String) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.equalizer.eq_selected_preset = preset;
    })
}
