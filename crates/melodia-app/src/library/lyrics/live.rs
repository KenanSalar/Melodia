//! Lyrics for a song a station announced.
//!
//! **Held in memory and nowhere else.** The store is keyed by a track's path and a song on air has
//! none, and a station plays thousands of songs nobody will hear again, so written down they would
//! be a directory growing without anything to sweep it against. The cache exists for rotation: a
//! station repeating its playlist inside a session asks once per song, misses included. It is
//! dropped whole when the station stops being the one playing, and its answers live no longer.
//!
//! Only the lookup can answer, so with it switched off a station has no lyrics at all. A timed
//! sheet stays timed here; whether a song's start is known enough to follow it is the panel's call.

use std::num::NonZeroUsize;

use lru::LruCache;
use parking_lot::Mutex;

use super::{ensure_enabled, lrc, online_lookup_enabled, romanized};
use crate::state::AppState;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::lyrics::{LyricsOutcome, LyricsSource};
use melodia_core::error::AppError;
use melodia_net::services::net::lyrics_directory as directory;

/// How many songs one listening session keeps answers for. Sized for a station's rotation, where
/// the answer worth keeping is the song that comes round again within the hour.
const CAPACITY: NonZeroUsize = match NonZeroUsize::new(32) {
    Some(n) => n,
    None => panic!("CAPACITY > 0"),
};

/// A song as the station named it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeardSong {
    pub artist: String,
    pub title: String,
}

/// What the directory said about the songs heard since the station was tuned.
pub struct LiveLyrics(Mutex<LruCache<HeardSong, LyricsOutcome>>);

impl Default for LiveLyrics {
    fn default() -> Self {
        Self(Mutex::new(LruCache::new(CAPACITY)))
    }
}

/// The sheet for a song on air, from the cache or the directory.
pub async fn for_live(state: &AppState, song: &HeardSong) -> Result<LyricsOutcome, AppError> {
    ensure_enabled(state)?;
    if !online_lookup_enabled(state) {
        return Ok(LyricsOutcome::Absent);
    }
    if let Some(known) = state.live_lyrics.0.lock().get(song).cloned() {
        return Ok(known);
    }

    let outcome = romanized(state, look_up(state, song).await).await?;
    // A lookup that could not be made is not an answer, and caching it would hide the song's
    // lyrics for the rest of the session on the strength of one bad minute.
    if outcome != LyricsOutcome::Unavailable {
        state.live_lyrics.0.lock().put(song.clone(), outcome.clone());
    }
    Ok(outcome)
}

/// Drop every answer held, for a station that has stopped playing.
pub fn forget_live(state: &AppState) {
    state.live_lyrics.0.lock().clear();
}

async fn look_up(state: &AppState, song: &HeardSong) -> LyricsOutcome {
    let credit = ArtistCredit::from_name(&song.artist);
    let answer = match directory::fetch_heard(
        state.http_client(),
        &state.lyrics_pacer,
        &song.title,
        &credit,
    )
    .await
    {
        Ok(answer) => answer,
        Err(e) => {
            log::debug!("lyrics: {e}");
            return LyricsOutcome::Unavailable;
        }
    };

    let Some(answer) = answer else {
        return LyricsOutcome::Absent;
    };
    if let Some(text) = answer.text() {
        return lrc::parse(text, LyricsSource::Online)
            .map_or(LyricsOutcome::Absent, LyricsOutcome::Sheet);
    }
    if answer.instrumental { LyricsOutcome::Instrumental } else { LyricsOutcome::Absent }
}
