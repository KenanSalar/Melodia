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
/// Deliberately not a `TrackSummary` with empty fields: a station has no duration, no album and no
/// id in `tracks`, and every surface that reads one would have to learn which of its fields to
/// disbelieve. `station_id` is `0` for a station the user has only browsed to, matching what
/// `views.json` can and cannot persist a detail id for.
///
/// **Held behind an `Arc`**, for `current_track`'s reason: it is cloned into the view model on
/// every state emit, and a volume drag emits per pointer move. That is also what makes the facts
/// below free to carry — the two writers that mutate it in place go through `Arc::make_mut` and
/// pay one clone a song rather than one an emit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioNowPlaying {
    pub station_id: i64,
    /// radio-browser.info's id, or `None` for a station the user typed in — which is what decides
    /// whether the directory can be told anything about it, a vote included.
    pub station_uuid: Option<String>,
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

    // What the Now-Playing surfaces *state* about the station, as against what the machine needs
    // to play it. Carried here rather than looked up per surface because the row they come from
    // is already in hand at the tune, and a second published description of the playing station
    // would be two answers to keep in step. Each is read through `RadioStation`'s override
    // accessor, so what the bar draws and what the station's own page draws cannot disagree.
    pub country: Option<String>,
    /// The directory's free-form comma-separated list, verbatim. The UI layer is what trims and
    /// joins it into a line, through the same helper a station card uses.
    pub tags: Option<String>,
    pub homepage: Option<String>,
    pub codec: Option<String>,
    /// Advertised kbps, `0` where the directory does not know — a display hint, never a divisor.
    pub bitrate: i32,
    pub play_count: i32,
}

impl From<&crate::entities::radio::RadioStation> for RadioNowPlaying {
    /// What a stored station looks like to the player: everything a Now-Playing surface draws, and
    /// none of the popularity, codec or favourite bookkeeping that belongs to the library views.
    fn from(station: &crate::entities::radio::RadioStation) -> Self {
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
            codec: (!station.codec.is_empty()).then(|| station.codec.clone()),
            bitrate: station.bitrate,
            play_count: station.play_count,
        }
    }
}

/// What is on the deck, and what may be done with it.
///
/// It replaces a pair of `Option`s — a track and a station — that were mutually exclusive by an
/// invariant nothing enforced, and that only one function ever restored. Making the exclusion
/// structural is the smaller half of what this buys.
///
/// The larger half is the accessors below. Source kind used to be a boolean, and a dozen branches
/// asked it as a stand-in for a dozen different questions: whether there is a next item, whether a
/// position can be asked for, whether a length is known, whether the speed may be varied. Podcasts
/// and streaming turn each of those into a three or four way question, and every site left as a
/// boolean silently answers it the way a local file would. Asking the capability instead means a
/// third variant states its own answers once rather than being audited into a dozen branches.
///
/// So the variants are not a taxonomy of where bytes come from. A podcast episode is far closer to
/// a local file than to a station — finite, seekable, with a position worth resuming — and it is
/// radio that is the odd one out.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackSource {
    Track(std::sync::Arc<crate::entities::track::TrackSummary>),
    Station(std::sync::Arc<RadioNowPlaying>),
}

impl PlaybackSource {
    /// The track, when this is one. `None` for every live source.
    pub fn track(&self) -> Option<&std::sync::Arc<crate::entities::track::TrackSummary>> {
        match self {
            Self::Track(track) => Some(track),
            Self::Station(_) => None,
        }
    }

    /// The track for in-place mutation, which is how a rating or a favourite reaches the deck.
    pub fn track_mut(
        &mut self,
    ) -> Option<&mut std::sync::Arc<crate::entities::track::TrackSummary>> {
        match self {
            Self::Track(track) => Some(track),
            Self::Station(_) => None,
        }
    }

    /// The station, when this is one.
    pub fn station(&self) -> Option<&std::sync::Arc<RadioNowPlaying>> {
        match self {
            Self::Station(station) => Some(station),
            Self::Track(_) => None,
        }
    }

    /// The station for in-place mutation, which is how a live title and the buffering flag arrive.
    pub fn station_mut(&mut self) -> Option<&mut std::sync::Arc<RadioNowPlaying>> {
        match self {
            Self::Station(station) => Some(station),
            Self::Track(_) => None,
        }
    }

    /// Whether the queue is what says which item follows this one.
    ///
    /// False for a station, whose queue is left seated underneath rather than played from: skipping
    /// into it would be a silent change of source, and a station going off air stops rather than
    /// advancing. Shortwave, Tuner and `RadioDroid` all disable both transports for the same reason.
    pub fn advances_queue(&self) -> bool {
        matches!(self, Self::Track(_))
    }

    /// Whether a position within the source can be asked for.
    pub fn is_seekable(&self) -> bool {
        matches!(self, Self::Track(_))
    }

    /// Whether the source knows how long it runs for, which is what a progress bar, a crossfade and
    /// an end-of-track sleep timer each need before they mean anything.
    pub fn has_known_duration(&self) -> bool {
        matches!(self, Self::Track(_))
    }

    /// Whether playback speed may be varied.
    ///
    /// rodio implements speed by reporting a multiplied sample rate upward, which against a
    /// fixed-rate live source drifts the ring until it starves — so a station is pinned to 1.0
    /// rather than merely discouraged from moving.
    pub fn has_variable_speed(&self) -> bool {
        matches!(self, Self::Track(_))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistableQueue {
    pub track_ids: Vec<i64>,
    pub current_index: i32,
}

/// What `queue.json` holds: the queue, and the station tuned over it.
///
/// The two are not alternatives. A station leaves the queue untouched underneath it (D9), so a
/// restart puts back both and a stop hands the library back exactly as one mid-session does.
///
/// **Flattened, so the file keeps the shape it has already shipped** — a `queue.json` written
/// before the station rode along still parses, and restores no station.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedPlayback {
    #[serde(flatten)]
    pub queue: PersistableQueue,
    /// The station on the deck, by row id, which is what the restore looks back up. `None` for a
    /// track, and for a station with no row of its own — [`RadioNowPlaying::station_id`] is `0`
    /// there, and nothing could be fetched with it.
    #[serde(default)]
    pub station_id: Option<i64>,
}

#[cfg(test)]
#[path = "tests/types_tests.rs"]
mod tests;
