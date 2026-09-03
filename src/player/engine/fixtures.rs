//! Fixtures for the transport suites, built out of this crate's own published types.
//!
//! `#[doc(hidden)] pub` rather than `#[cfg(test)]`, and not by preference: three crates read
//! these — the engine's own suites, `library`'s playback tests and the now-playing ladder under
//! `ui/` — and a `cfg(test)` item cannot cross a crate boundary. `DbPool::test_pool` is the same
//! shape for the same reason. The cheaper fixtures each tier can spell for itself stayed
//! `cfg(test)` beside their tests; these three could not, `RadioNowPlaying` having thirteen
//! fields and `PlayerViewModelLight` thirteen more.

use std::sync::Arc;

use crate::entities::track::TrackSummary;
use crate::player::engine::state::PlayerViewModelLight;
use crate::player::engine::types::RadioNowPlaying;

/// A station for the transport tests, with only what they assert on filled in.
///
/// Four suites were spelling this out — three under `player/` and one under `library/` — so a
/// field added to `RadioNowPlaying` broke all four the same way. The display facts are left empty
/// deliberately: nothing below the UI layer reads them, so a fixture carrying them would suggest
/// the transport cares.
pub fn test_station(name: &str) -> Arc<RadioNowPlaying> {
    Arc::new(RadioNowPlaying {
        station_id: 42,
        station_uuid: None,
        name: name.to_owned(),
        stream_url: "http://example.test/live.mp3".to_owned(),
        artwork_path: None,
        live_title: None,
        buffering: false,
        country: None,
        tags: None,
        homepage: None,
        codec: None,
        bitrate: 0,
        play_count: 0,
    })
}

/// A track for the suites that need one on the deck rather than a decodable file, with only the
/// tagged fields varying. Shares [`test_station`]'s reason for existing: `TrackSummary` has
/// seventeen columns and the two suites reading this one care about four.
pub fn test_track(title: &str, artist: Option<&str>, album: Option<&str>) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id: 7,
        file_path: String::new(),
        file_name: String::new(),
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        album: album.map(str::to_owned),
        duration_ms: 200_000,
        artwork_path: Some("cover.jpg".to_owned()),
        track_number: None,
        disc_number: None,
        last_position: 0,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    })
}

/// A published view model carrying `current_track`, `radio` and the player's own `duration_ms`,
/// which is the trio anything asking what is on the deck reads. Everything else is left at a
/// resting value: a test that wants one sets it on the returned struct, so a field added to
/// `PlayerViewModelLight` lands here rather than in every suite that builds one.
pub fn test_view_model(
    current_track: Option<Arc<TrackSummary>>,
    radio: Option<Arc<RadioNowPlaying>>,
    duration_ms: u64,
) -> PlayerViewModelLight {
    PlayerViewModelLight {
        status: "playing",
        current_track,
        position_ms: 0,
        duration_ms,
        progress_percent: 0.0,
        volume: 100,
        is_muted: false,
        playback_speed: 1.0,
        gapless_enabled: false,
        sleep_at_track_end: false,
        radio,
        has_next: false,
        has_previous: false,
    }
}
