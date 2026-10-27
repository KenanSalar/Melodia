use std::borrow::Cow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use melodia_core::entities::track::TrackSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Stopped,
    Playing,
    Paused,
    /// Nothing audible yet: a station connecting, or a track start waiting on its record. Never a
    /// rebuffer, which stays `Playing` and raises `StreamShared::is_buffering`, since the OS
    /// reporting sites map this one to stopped.
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

/// The station the player is tuned to: its own type rather than a `TrackSummary` with empty
/// fields, which every surface would have to learn to disbelieve. `station_id` is `0` for a
/// station only browsed to.
///
/// Behind an `Arc` for `current_track`'s reason, cloned into the view model on every emit; its
/// two in-place writers pay one clone a song through `Arc::make_mut`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioNowPlaying {
    pub station_id: i64,
    /// radio-browser.info's id, `None` for a station typed in, which the directory can't be told
    /// about.
    pub station_uuid: Option<String>,
    pub name: String,
    pub stream_url: String,
    pub artwork_path: Option<String>,
    /// As the stream announces it, `None` until its first metadata block.
    pub live_title: Option<String>,
    /// Reconciled by the monitor; here rather than on `PlayerState`, meaning nothing without a
    /// station.
    pub buffering: bool,

    // What the Now Playing surfaces state about the station, carried from the row in hand at the
    // tune and read through `RadioStation`'s accessors, so the bar and the station's page agree.
    pub country: Option<String>,
    /// The directory's comma-separated list, verbatim; the UI trims and joins it.
    pub tags: Option<String>,
    pub homepage: Option<String>,
    pub codec: Option<String>,
    /// Advertised kbps, `0` where the directory does not know — a display hint, never a divisor.
    pub bitrate: i32,
    pub play_count: i32,
}

impl From<&melodia_core::entities::radio::RadioStation> for RadioNowPlaying {
    fn from(station: &melodia_core::entities::radio::RadioStation) -> Self {
        Self {
            station_id: station.id,
            station_uuid: station.station_uuid.clone(),
            name: station.name.clone(),
            stream_url: station.stream_url.clone(),
            artwork_path: station.artwork_path.clone(),
            live_title: None,
            buffering: false,
            country: station.country_name().map(str::to_owned),
            tags: station.genre().map(str::to_owned),
            homepage: station.website().map(str::to_owned),
            codec: station.format().map(Cow::into_owned),
            bitrate: station.bitrate,
            play_count: station.play_count,
        }
    }
}

/// What is on the deck, and what may be done with it.
///
/// Callers ask a capability rather than the variant, so a third kind of source (a podcast sits far
/// closer to a file than to a station) states its answers once instead of being audited into
/// every branch.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackSource {
    Track(Arc<TrackSummary>),
    Station(Arc<RadioNowPlaying>),
}

impl PlaybackSource {
    pub fn track(&self) -> Option<&Arc<TrackSummary>> {
        match self {
            Self::Track(track) => Some(track),
            Self::Station(_) => None,
        }
    }

    pub fn track_mut(&mut self) -> Option<&mut Arc<TrackSummary>> {
        match self {
            Self::Track(track) => Some(track),
            Self::Station(_) => None,
        }
    }

    pub fn station(&self) -> Option<&Arc<RadioNowPlaying>> {
        match self {
            Self::Station(station) => Some(station),
            Self::Track(_) => None,
        }
    }

    pub fn station_mut(&mut self) -> Option<&mut Arc<RadioNowPlaying>> {
        match self {
            Self::Station(station) => Some(station),
            Self::Track(_) => None,
        }
    }

    /// Whether the queue says what follows. A station's queue stays seated underneath, and one
    /// going off air stops rather than advancing into it.
    pub fn advances_queue(&self) -> bool {
        matches!(self, Self::Track(_))
    }

    pub fn is_seekable(&self) -> bool {
        matches!(self, Self::Track(_))
    }

    /// What a progress bar, a crossfade and an end-of-track sleep timer each need.
    pub fn has_known_duration(&self) -> bool {
        matches!(self, Self::Track(_))
    }

    /// A live mount arrives at exactly real time, so a station is pinned to 1.0 or its ring
    /// starves or overruns.
    pub fn has_variable_speed(&self) -> bool {
        matches!(self, Self::Track(_))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistableQueue {
    pub track_ids: Vec<i64>,
    pub current_index: i32,
}

/// What `queue.json` holds: the queue and the station tuned over it, which leaves the queue
/// untouched, so a restart puts back both. Flattened, so a file from before the station rode
/// along still parses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedPlayback {
    #[serde(flatten)]
    pub queue: PersistableQueue,
    /// `None` for a track, and for a station with no row of its own.
    #[serde(default)]
    pub station_id: Option<i64>,
}

#[cfg(test)]
#[path = "tests/types_tests.rs"]
mod tests;
