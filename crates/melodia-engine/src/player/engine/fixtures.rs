//! Fixtures for the transport suites. `#[doc(hidden)] pub` rather than `#[cfg(test)]` because
//! suites in other crates read them, as `DbPool::test_pool` is.

use std::sync::Arc;

use crate::player::engine::state::PlayerViewModelLight;
use crate::player::engine::types::{RadioNowPlaying, RepeatMode};
use melodia_core::entities::track::TrackSummary;

/// A station with only what the transport tests assert on. The display facts stay empty, nothing
/// below the UI reading them.
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

/// A track for suites needing one on the deck rather than a decodable file.
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

/// A view model carrying the trio that says what is on the deck, the rest at rest, so a new field
/// lands here rather than in every suite.
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
        position_stamp: 0,
        progress_percent: 0.0,
        volume: 100,
        is_muted: false,
        playback_speed: 1.0,
        gapless_enabled: false,
        sleep_at_track_end: false,
        radio,
        has_next: false,
        has_previous: false,
        shuffle_enabled: false,
        repeat_mode: RepeatMode::Off,
    }
}
