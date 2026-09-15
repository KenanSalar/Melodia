//! The side effects a state transition hands back, run once the lock is dropped.

use melodia_playback::player::playback::replaygain::TrackReplayGain;

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerAction {
    PlayMedia {
        file_path: String,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        /// This track's baked `ReplayGain` tag values, applied by the audio source.
        replaygain: TrackReplayGain,
    },
    /// Overlap the next track with the one still playing, fading between them
    /// over `fade_ms` **media** milliseconds. Unlike `PlayMedia` this leaves the
    /// current track audible; the backend runs the two on separate decks.
    BeginCrossfade {
        file_path: String,
        /// The *incoming* track's baked `ReplayGain` values. Baked per source —
        /// the outgoing track has its own, already applied.
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
    /// Move the playing track's position.
    ///
    /// Carries the file and its gain for the same reason [`Self::PlayMedia`] does: the backend
    /// seeks by building a source already at the target, so a seek rebuilds what the deck holds
    /// and the new source needs its own baked values. Only ever emitted for a track, a live
    /// source having no timeline to land on.
    Seek {
        position_ms: u64,
        file_path: String,
        replaygain: TrackReplayGain,
    },
    SetVolume(f64),
    SetSpeed(f64),
    PreloadGapless(Option<String>),
    /// Start the live stream the backend already has staged for this station session.
    ///
    /// Carries neither a URL nor a reader: opening a stream is a network round trip and belongs on
    /// the async task that started the station, not on the executor thread. `generation` is what
    /// the backend checks the stage against, so an action that outlived its session plays nothing.
    PlayStream {
        generation: u64,
        volume: f64,
    },
    UpdatePlayCount(i64),
    UpdateSkipCount(i64),
}

/// The verbose log's playback narrative, one action per line.
///
/// Terse where the derived `Debug` is exhaustive — `PlayMedia` alone would print
/// a whole `TrackReplayGain`. On the enum rather than in `execute_actions` so a
/// new variant is a non-exhaustive-match failure here, where a `_ =>` arm in the
/// executor would have logged it as nothing at all.
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
        }
    }
}
