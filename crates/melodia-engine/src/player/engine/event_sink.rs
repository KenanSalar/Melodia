use std::sync::Arc;

use tokio::sync::watch;

use super::state::{AwaitedStart, PlayerViewModelLight, QueueViewModel};
use super::types::{PlaybackStatus, RepeatMode};

/// Events that come *from* OS media controls (MPRIS, SMTC, `MediaPlayer`) into the player.
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
    SeekTo(u64),
    SetVolume(u32),
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    SetSpeed(f64),
}

pub trait EventSink: Send + Sync + 'static {
    fn handle(&self, ev: PlayerEvent);
}

pub struct NoopEventSink;
impl EventSink for NoopEventSink {
    fn handle(&self, _: PlayerEvent) {}
}

/// The OS media controls' push direction, synced by `with_state_emit` after every mutation.
pub trait MediaControlsSync: Send + Sync + 'static {
    fn sync(&self, vm: &PlayerViewModelLight, status: PlaybackStatus);

    /// The playing position, on every poll. Cheap by contract: a backend that pays per push
    /// throttles itself, since only it knows what a push costs.
    fn update_position(&self, _position_ms: u64) {}

    /// The position jumped rather than advanced. MPRIS clients extrapolate between reads and
    /// learn of a jump only through this.
    fn seeked(&self, _position_ms: u64) {}
}

/// What `with_state_emit` publishes into, `Option` payloads because there is no meaningful view
/// model at startup.
pub struct PlayerSinks {
    pub view_model: watch::Sender<Option<PlayerViewModelLight>>,
    pub queue: watch::Sender<Option<QueueViewModel>>,
    pub media_controls: Option<Arc<dyn MediaControlsSync>>,
    /// The newest track start owed to a record the window lacked, for the app's window task to
    /// fetch and run. Only the newest matters: a later transition supersedes an earlier one.
    pub awaited_start: watch::Sender<Option<AwaitedStart>>,
}
