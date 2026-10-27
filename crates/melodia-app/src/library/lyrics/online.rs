//! The lyrics directory, and the only thing in this feature that opens a socket.

use super::lrc;
use super::store::{self, Fetched};
use crate::state::AppState;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::lyrics::{LyricsOutcome, LyricsSource};
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::text::filled;
use melodia_net::services::net::lyrics_directory as directory;
use melodia_store::database::{DbPool, queries};

/// Looks a sheet up for a track that carries none of its own, and records what came back. Skipped
/// where it could only miss, the directory naming a recording by artist, title and duration.
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
        &credit_for(&state.db, track.id, artist).await,
        filled(track.album.as_deref()).unwrap_or_default(),
        track.duration_ms,
    )
    .await
    {
        Ok(answer) => answer,
        // A refusal or an outage is no answer at all: nothing is stored, and the panel is told we
        // couldn't ask.
        Err(e) => {
            log::debug!("lyrics: {}", describe(&e));
            return Ok(LyricsOutcome::Unavailable);
        }
    };

    let text = answer.as_ref().and_then(|answer| answer.text());
    let instrumental = answer.as_ref().is_some_and(|answer| answer.instrumental);

    // The nothing is recorded too, or the next replay pays the request again.
    let fetched = match (text, instrumental) {
        (Some(text), _) => Fetched::Sheet(text),
        (None, true) => Fetched::Instrumental,
        (None, false) => Fetched::Nothing,
    };
    if let Err(e) = store::write(&state.paths.lyrics_dir, &track.file_path, fetched) {
        // Costs a request next time, never the sheet in hand.
        log::debug!("lyrics: not stored: {}", melodia_core::error::describe(&e));
    }

    // Off what was stored, so the panel shows what the next replay will read back.
    Ok(match fetched {
        Fetched::Sheet(text) => lrc::parse(text, LyricsSource::Online)
            .map_or(LyricsOutcome::Absent, LyricsOutcome::Sheet),
        Fetched::Instrumental => LyricsOutcome::Instrumental,
        Fetched::Nothing => LyricsOutcome::Absent,
    })
}

/// The credit behind the printed artist line, taken only where it renders back to `printed`:
/// anything else is stale, `tasks::tag_backfill` not having reached that row's files yet. The
/// fallback costs the lookup precision rather than its answer.
async fn credit_for(db: &DbPool, track_id: i64, printed: &str) -> ArtistCredit {
    match queries::track::get_track_credit(db, track_id).await {
        Ok(credit) if credit.line() == Some(printed) => credit,
        Ok(_) => ArtistCredit::from_name(printed),
        Err(e) => {
            log::debug!("lyrics: credit unread: {}", melodia_core::error::describe(&e));
            ArtistCredit::from_name(printed)
        }
    }
}

#[cfg(test)]
#[path = "tests/online_tests.rs"]
mod tests;
