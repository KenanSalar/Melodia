use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Stopped,
    Playing,
    Paused,
    /// Nothing is audible yet: a station is being connected to. Deliberately **not** what a
    /// rebuffer or a reconnect reports — those keep `Playing` and raise
    /// [`crate::player::prebuffer::StreamShared::is_buffering`] instead, because the two OS
    /// reporting sites map this to "stopped" and a station dipping its buffer has not stopped.
    Loading,
}

impl PlaybackStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlaybackStatus::Stopped => "stopped",
            PlaybackStatus::Playing => "playing",
            PlaybackStatus::Paused => "paused",
            PlaybackStatus::Loading => "loading",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::All => "all",
            RepeatMode::One => "one",
        }
    }

    /// True when manual next/previous wraps and Up Next renders a wrapping slice.
    pub fn wraps(self) -> bool {
        matches!(self, Self::All | Self::One)
    }
}

/// The station the player is tuned to, which is the whole of what a live source has where a track
/// has a [`crate::entities::track::TrackSummary`].
///
/// Deliberately not a `TrackSummary` with empty fields: a station has no duration, no album, no
/// tags and no id in `tracks`, and every surface that reads one would have to learn which of its
/// fields to disbelieve. `station_id` is `0` for a station the user has only browsed to, matching
/// what `views.json` can and cannot persist a detail id for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioNowPlaying {
    pub station_id: i64,
    pub name: String,
    pub stream_url: String,
    pub artwork_path: Option<String>,
    /// The current track as the stream itself announces it, in whatever shape the station sends.
    /// `None` until the first metadata block arrives, which for some stations is never.
    pub live_title: Option<String>,
    /// Whether the stream is currently running on empty. It lives here rather than on
    /// `PlayerState` because it means nothing without a station: reconciled by the playback
    /// monitor off [`crate::player::prebuffer::StreamShared`], and gone the moment this is.
    pub buffering: bool,
}

impl From<&crate::entities::radio::RadioStation> for RadioNowPlaying {
    /// What a stored station looks like to the player: everything a Now-Playing surface draws, and
    /// none of the popularity, codec or favourite bookkeeping that belongs to the library views.
    fn from(station: &crate::entities::radio::RadioStation) -> Self {
        Self {
            station_id: station.id,
            name: station.name.clone(),
            stream_url: station.stream_url.clone(),
            artwork_path: station.artwork_path.clone(),
            live_title: None,
            buffering: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistableQueue {
    pub track_ids: Vec<i64>,
    pub current_index: i32,
}

#[cfg(test)]
#[path = "tests/types_tests.rs"]
mod tests;
