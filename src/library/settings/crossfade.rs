//! Crossfade-section setters. Like the equalizer and `ReplayGain` setters, the
//! runtime effect (applying to the live playback engine) is done synchronously by
//! the UI callback through `library::playback::player_set_crossfade_*` *before*
//! these async disk writes, so the change takes effect immediately and the
//! `settings.json` commit only persists the choice. Mirrors the shape of
//! [`super::replaygain`].

use crate::error::AppError;
use crate::player::crossfade::clamp_crossfade_ms;
use crate::services;
use crate::state::AppState;

/// Persist the crossfade on/off toggle. Defaults to `false` on first launch
/// (`CrossfadeFlags::default()`), so new installs keep plain gapless playback.
pub fn set_crossfade_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.crossfade.crossfade_enabled = enabled;
    })
}

/// Persist the crossfade length (ms). Clamped here too so a hand-edited
/// `settings.json` can't pin an out-of-range value.
pub fn set_crossfade_duration_ms(state: &AppState, ms: u32) -> Result<(), AppError> {
    let ms = clamp_crossfade_ms(ms);
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.crossfade.crossfade_duration_ms = ms;
    })
}

/// Persist whether a manual track change (next / previous / picking a track)
/// also crossfades, rather than cutting straight over.
pub fn set_crossfade_manual(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.crossfade.crossfade_manual = on;
    })
}

/// Persist the same-album exception. Defaults **on** so continuous-mix albums
/// keep their gapless transitions once crossfade is enabled.
pub fn set_crossfade_skip_same_album(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.crossfade.crossfade_skip_same_album = on;
    })
}

/// Persist the fade-out-on-pause toggle (also covers user-initiated stop, and
/// the matching fade back in on resume).
pub fn set_crossfade_fade_on_pause(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.crossfade.crossfade_fade_on_pause = on;
    })
}
