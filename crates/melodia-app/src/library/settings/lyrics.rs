//! The lyrics feature's three switches.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Also whether the Now Playing column shows lyrics instead of Up Next.
pub fn set_lyrics_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.lyrics.lyrics_enabled = enabled;
    })
}

/// Whether a track with no sheet of its own may be looked up online.
pub fn set_lyrics_online_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.lyrics.lyrics_online_enabled = enabled;
    })
}

/// **The one writer, with two callers**, the Settings card and the Now Playing menu: a second
/// `mutate_settings` is how the two would come to disagree about the field.
pub fn set_lyrics_romanization_shown(paths: &Paths, shown: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.lyrics.lyrics_romanization_shown = shown;
    })
}

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
