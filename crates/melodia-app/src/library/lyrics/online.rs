//! The lyrics directory, and the only thing in this feature that opens a socket.

use super::lrc;
use crate::state::AppState;
use melodia_core::entities::lyrics::{Lyrics, LyricsSource};
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_net::services::net::lrclib;

/// Looks a sheet up for a track that carries none of its own.
///
/// **The request is skipped where it could only miss.** The directory identifies a recording by
/// artist, title and duration together, so a track with no artist tag, or one whose duration never
/// made it out of the scan, is a guaranteed miss and asking is traffic spent to learn nothing.
pub(super) async fn look_up(
    state: &AppState,
    track: &TrackSummary,
) -> Result<Option<Lyrics>, AppError> {
    let Some(artist) = filled(track.artist.as_deref()) else {
        return Ok(None);
    };
    if track.duration_ms <= 0 {
        return Ok(None);
    }

    let answer = lrclib::fetch(
        state.http_client(),
        &track.title,
        artist,
        filled(track.album.as_deref()).unwrap_or_default(),
        track.duration_ms / 1000,
    )
    .await?;

    // An instrumental is a real answer and deserves its own copy in the panel, but nothing can
    // carry that yet: `Lyrics` is a sheet, and the outcome type telling "no words" apart from
    // "nothing found" arrives with the panel that draws the difference.
    let Some(answer) = answer else {
        return Ok(None);
    };
    let Some(text) = answer.text() else {
        return Ok(None);
    };
    Ok(lrc::parse(text, LyricsSource::Online))
}

/// A tag field that is actually there, rather than present and empty.
fn filled(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|value| !value.is_empty())
}
