//! The lyrics panel's two switches. Persist to `settings.json`; the shadow on [`AppState`] for
//! the online one is written synchronously by the UI callback *before* the persist is spawned, so
//! a track change racing the disk write reads the new answer rather than the old file.

use crate::services;
use crate::state::AppState;
use melodia_core::error::AppError;

/// Persist whether the Now Playing column shows lyrics instead of Up Next.
pub fn set_lyrics_panel_shown(state: &AppState, shown: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.lyrics.lyrics_panel_shown = shown;
    })
}

/// Persist whether a track with no sheet of its own may be looked up online. Off by default
/// (opt-in), for the reason every other outbound feature is.
pub fn set_lyrics_online_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.lyrics.lyrics_online_enabled = enabled;
    })
}
