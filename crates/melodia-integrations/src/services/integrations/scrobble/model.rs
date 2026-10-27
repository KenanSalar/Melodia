//! The provider-agnostic scrobble payload and the threshold rule both services share.

use melodia_engine::player::engine::now_playing::Announcement;
use serde::{Deserialize, Serialize};

/// What both services accept, persisted verbatim in the offline queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrobbleTrack {
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration_secs: Option<u32>,
    pub track_number: Option<u32>,
    /// `MusicBrainz` recording id (`musicbrainz_track_id`) — LB `recording_mbid`.
    pub recording_mbid: Option<String>,
    /// `MusicBrainz` release id (`musicbrainz_release_id`) — LB `release_mbid`.
    pub release_mbid: Option<String>,
}

impl ScrobbleTrack {
    /// `None` when the row can't scrobble, both services needing an artist and a title.
    pub fn from_row(row: &melodia_core::entities::track::ScrobbleRow) -> Option<Self> {
        let artist = non_empty(row.artist.as_deref())?;
        let track = non_empty(Some(&row.title))?;
        Some(Self {
            artist,
            track,
            album: non_empty(row.album.as_deref()),
            album_artist: non_empty(row.album_artist.as_deref()),
            duration_secs: u32::try_from(row.duration_ms / 1000).ok().filter(|&s| s > 0),
            track_number: row.track_number.and_then(|n| u32::try_from(n).ok()),
            recording_mbid: non_empty(row.musicbrainz_track_id.as_deref()),
            release_mbid: non_empty(row.musicbrainz_release_id.as_deref()),
        })
    }

    /// A stream carries nothing past the two names, so every other field stays absent rather than
    /// guessed.
    pub fn heard_on_air(song: Announcement<'_>) -> Self {
        Self {
            artist: song.artist.to_owned(),
            track: song.title.to_owned(),
            album: None,
            album_artist: None,
            duration_secs: None,
            track_number: None,
            recording_mbid: None,
            release_mbid: None,
        }
    }
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
}

/// The length both services require a track to exceed before it may scrobble at all.
pub const MIN_TRACK_MS: u64 = 30_000;

/// Played time after which a track over [`MIN_TRACK_MS`] qualifies: half its length or four
/// minutes, whichever comes first. `None` for one that never can; an unknown length (`0`) takes
/// the four-minute cap.
pub fn scrobble_threshold_ms(duration_ms: u64) -> Option<u64> {
    const FOUR_MIN_MS: u64 = 240_000;

    if duration_ms == 0 {
        return Some(FOUR_MIN_MS);
    }
    if duration_ms <= MIN_TRACK_MS {
        return None;
    }
    Some((duration_ms / 2).min(FOUR_MIN_MS))
}

#[cfg(test)]
#[path = "tests/model_tests.rs"]
mod tests;
