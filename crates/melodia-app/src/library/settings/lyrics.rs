//! The lyrics panel's three switches. Persist to `settings.json`; the shadows on [`AppState`] are
//! written synchronously by the UI callback *before* the persist is spawned, so a reader racing the
//! disk write sees the new answer rather than the old file.
//!
//! **Each setter is one line over a narrowed writer**, as `settings::view` does it, so which field
//! a switch assigns can be driven without an `AppState`.

use crate::services;
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Persist whether the Now Playing column shows lyrics instead of Up Next.
pub fn set_lyrics_panel_shown(state: &AppState, shown: bool) -> Result<(), AppError> {
    write_lyrics_panel_shown(&state.paths, shown)
}

fn write_lyrics_panel_shown(paths: &Paths, shown: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.lyrics.lyrics_panel_shown = shown;
    })
}

/// Persist whether a track with no sheet of its own may be looked up online. Off by default
/// (opt-in), for the reason every other outbound feature is.
pub fn set_lyrics_online_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    write_lyrics_online_enabled(&state.paths, enabled)
}

fn write_lyrics_online_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.lyrics.lyrics_online_enabled = enabled;
    })
}

/// Persist whether the panel draws the romanization under each line.
///
/// **The one writer, and it has two callers**: the Settings card and the Now Playing menu. A
/// second `mutate_settings` beside it is how the two would come to disagree about the field's
/// name, which is the same argument `set_lyrics_panel_shown` makes for having only one.
pub fn set_lyrics_romanization_shown(state: &AppState, shown: bool) -> Result<(), AppError> {
    write_lyrics_romanization_shown(&state.paths, shown)
}

fn write_lyrics_romanization_shown(paths: &Paths, shown: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.lyrics.lyrics_romanization_shown = shown;
    })
}

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
