//! The side effects a state transition hands back, run once the lock is dropped.

use melodia_playback::player::playback::replaygain::TrackReplayGain;

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerAction {
    PlayMedia {
        file_path: String,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        replaygain: TrackReplayGain,
    },
    /// Fades over `fade_ms` media milliseconds into the next track on the other deck, the current
    /// one staying audible.
    BeginCrossfade {
        file_path: String,
        /// The incoming track's own.
        replaygain: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    },
    Resume,
    /// `fade_ms` is the pause-fade length for a user-initiated pause, and `0` where a fade would be
    /// wrong: see `PlayerState::restore_paused`.
    Pause {
        fade_ms: u64,
    },
    /// `fade_ms` is `0` for an internal stop (end of queue, error recovery) and
    /// the pause-fade length for a user-initiated stop.
    Stop {
        fade_ms: u64,
    },
    /// Carries the file and its gain because the backend seeks by building a source already at
    /// the target. Only ever for a track.
    Seek {
        position_ms: u64,
        file_path: String,
        replaygain: TrackReplayGain,
    },
    SetVolume(f64),
    SetSpeed(f64),
    PreloadGapless(Option<String>),
    /// Starts the stream the backend staged for session `generation`; the open, a network round
    /// trip, belongs to the async task that started the station.
    PlayStream {
        generation: u64,
        volume: f64,
    },
    UpdatePlayCount(i64),
    UpdateSkipCount(i64),
    /// Start a queued track once its record is fetched, the window not holding one yet. Carries no
    /// fetch of its own: the engine can't name the database, so the app's window task does it.
    StartWhenResolved(AwaitedStart),
}

/// A track start a transition owes, waiting on a record the window lacked. It runs on whatever the
/// queue's current slot holds once the record lands, so an edit during the fetch moves what starts
/// rather than stranding the player under `Loading`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwaitedStart {
    /// The track the transition moved onto, which is what the log names.
    pub id: i64,
    pub re_pause: bool,
    /// So a lap of unplayable tracks still stops however many fetches it took.
    pub skipped: usize,
}

/// The verbose log's narrative: terser than `Debug`, and on the enum so a new variant fails to
/// match here rather than logging as nothing.
impl std::fmt::Display for PlayerAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlayMedia { file_path, start_position_ms, .. } => match start_position_ms {
                Some(ms) => write!(f, "play {file_path} from {ms}ms"),
                None => write!(f, "play {file_path}"),
            },
            Self::BeginCrossfade { file_path, fade_ms, .. } => {
                write!(f, "crossfade {fade_ms}ms into {file_path}")
            }
            Self::Resume => f.write_str("resume"),
            Self::Pause { fade_ms } => write!(f, "pause (fade {fade_ms}ms)"),
            Self::Stop { fade_ms } => write!(f, "stop (fade {fade_ms}ms)"),
            Self::Seek { position_ms, .. } => write!(f, "seek to {position_ms}ms"),
            Self::SetVolume(v) => write!(f, "volume {v:.2}"),
            Self::SetSpeed(s) => write!(f, "speed {s:.2}"),
            Self::PreloadGapless(Some(path)) => write!(f, "preload gapless {path}"),
            Self::PreloadGapless(None) => f.write_str("clear gapless preload"),
            // No URL: a station's stream URL can carry a session token, and this line goes into
            // the log tail users attach to public issues.
            Self::PlayStream { generation, .. } => write!(f, "play radio stream #{generation}"),
            Self::UpdatePlayCount(id) => write!(f, "play count +1 for track {id}"),
            Self::UpdateSkipCount(id) => write!(f, "skip count +1 for track {id}"),
            Self::StartWhenResolved(awaited) => {
                write!(f, "start track {} once its record is fetched", awaited.id)
            }
        }
    }
}
