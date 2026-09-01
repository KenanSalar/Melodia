//! Equalizer-section setters. The runtime effect (applying gains to the live EQ)
//! is done synchronously by the UI callback through
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

/// Persist the preamp / master gain (dB). Clamped here too so a hand-edited
/// `settings.json` can't pin an out-of-range value. The runtime effect is
/// applied synchronously by the UI callback through
/// `library::playback::player_set_eq_preamp` before this disk write.
pub fn set_eq_preamp(state: &AppState, preamp_db: f32) -> Result<(), AppError> {
    let preamp_db = equalizer::clamp_preamp(preamp_db);
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.equalizer.eq_preamp = preamp_db;
    })
}

/// Persist the band gains **and** the selected-preset name in a single settings
/// write. Drag-release (`commit-band`), preset selection, and reset each change
/// both at once, so one `mutate_settings` read-modify-write replaces the two it
/// would otherwise take.
///
/// Gains are clamped + length-normalised here too, so a hand-edited or
/// wrong-length `settings.json` array can't pin a bad value. The preset name is
/// stored verbatim (a built-in name or the `"Custom"` sentinel the UI uses for a
/// hand-tuned curve); hydration falls back to a Custom display when the name
/// isn't one of the built-in presets.
pub fn set_eq_band_gains_and_preset(
    state: &AppState,
    gains: &[f32],
    preset: String,
) -> Result<(), AppError> {
    let norm = equalizer::normalize_gains(gains).to_vec();
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.equalizer.eq_band_gains = norm;
        settings.equalizer.eq_selected_preset = preset;
    })
}
