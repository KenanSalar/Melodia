//! The lyrics directory, and the only thing in this feature that opens a socket.

use super::lrc;
use super::store::{self, Fetched};
use crate::state::AppState;
use melodia_core::entities::lyrics::{LyricsOutcome, LyricsSource};
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_net::services::net::lyrics_directory as directory;

/// Looks a sheet up for a track that carries none of its own, and records what came back.
///
/// **The request is skipped where it could only miss.** The directory identifies a recording by
/// artist, title and duration together, so a track with no artist tag, or one whose duration never
/// made it out of the scan, is a guaranteed miss and asking is traffic spent to learn nothing.
pub(super) async fn look_up(
    state: &AppState,
    track: &TrackSummary,
) -> Result<LyricsOutcome, AppError> {
    let Some(artist) = filled(track.artist.as_deref()) else {
        return Ok(LyricsOutcome::Absent);
    };
    if track.duration_ms <= 0 {
        return Ok(LyricsOutcome::Absent);
    }

    let answer = match directory::fetch(
        state.http_client(),
        &state.lyrics_pacer,
        &track.title,
        artist,
        filled(track.album.as_deref()).unwrap_or_default(),
        track.duration_ms,
    )
    .await
    {
        Ok(answer) => answer,
        // **Nothing is recorded and nothing is claimed.** A refusal and an outage are both the
        // absence of an answer rather than one, so the store stays untouched and the panel is told
        // we could not ask, which is the only honest thing left to say.
        Err(e) => {
            log::debug!("lyrics: {e}");
            return Ok(LyricsOutcome::Unavailable);
        }
    };

    let text = answer.as_ref().and_then(|answer| answer.text());
    let instrumental = answer.as_ref().is_some_and(|answer| answer.instrumental);

    // Recorded whatever it was, including the nothing: a miss the store did not keep is a request
    // paid again on the next replay, which is most of what this store exists to stop.
    let fetched = match (text, instrumental) {
        (Some(text), _) => Fetched::Sheet(text),
        (None, true) => Fetched::Instrumental,
        (None, false) => Fetched::Nothing,
    };
    if let Err(e) = store::write(&state.paths.lyrics_dir, &track.file_path, fetched) {
        // Losing the cache write costs a request next time and nothing else, so it must not cost
        // the sheet that is already in hand.
        log::debug!("lyrics: not stored: {}", melodia_core::error::describe(&e));
    }

    Ok(match (text, instrumental) {
        (Some(text), _) => lrc::parse(text, LyricsSource::Online)
            .map_or(LyricsOutcome::Absent, LyricsOutcome::Sheet),
        (None, true) => LyricsOutcome::Instrumental,
        (None, false) => LyricsOutcome::Absent,
    })
}

/// A tag field that is actually there, rather than present and empty.
fn filled(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "tests/online_tests.rs"]
mod tests;
