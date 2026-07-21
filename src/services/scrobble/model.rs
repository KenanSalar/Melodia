//! Provider-agnostic scrobble payload and the play-duration threshold rule
//! both services agree on. Kept free of any provider or I/O type so it stays a
//! pure, independently-testable core.

use serde::{Deserialize, Serialize};

/// One scrobble's worth of track metadata, normalized to what both Last.fm and
/// `ListenBrainz` accept. Enriched from a DB row at track start (Phase 2) and
/// persisted verbatim in the offline queue, so it derives `Serialize` /
/// `Deserialize` and owns its strings.
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

/// Played-time (ms) after which a track qualifies to scrobble, per the rule both
/// services share: a track over 30 s scrobbles once it has played at least half
/// its length **or** 4 minutes, whichever comes first.
///
/// Returns `None` for tracks that can never scrobble (30 s or shorter). A
/// `duration_ms` of `0` means "duration unknown"; those fall back to the 4-minute
/// cap so a stream with no reported length can still scrobble.
pub fn scrobble_threshold_ms(duration_ms: u64) -> Option<u64> {
    const FOUR_MIN_MS: u64 = 240_000;
    const MIN_TRACK_MS: u64 = 30_000;

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
