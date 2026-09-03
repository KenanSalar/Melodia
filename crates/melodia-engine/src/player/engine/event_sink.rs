use std::sync::Arc;

use tokio::sync::watch;

use super::state::{PlayerViewModelLight, QueueViewModel};
use super::types::PlaybackStatus;

/// Events that come *from* OS media controls (souvlaki) into the player.
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
}

pub trait EventSink: Send + Sync + 'static {
    fn handle(&self, ev: PlayerEvent);
}

pub struct NoopEventSink;
impl EventSink for NoopEventSink {
    fn handle(&self, _: PlayerEvent) {}
}

/// Push direction of OS media controls — `with_state_emit` calls `sync` after
/// every state mutation so MPRIS / SMTC stay in lockstep with the player.
/// `update_position` is the lighter periodic position refresh the playback
/// monitor uses on macOS / Windows. Implemented by `MediaControlsHandle`;
/// `PlayerSinks` holds `None` on platforms where no handle could be created.
pub trait MediaControlsSync: Send + Sync + 'static {
    fn sync(&self, vm: &PlayerViewModelLight, status: PlaybackStatus);
    fn update_position(&self, _position_ms: u64) {}
}

/// Sinks consumed by `with_state_emit`. The two watch senders carry the
/// reactive view-model updates the Slint bridge subscribes to;
/// `media_controls` is the optional push-direction hook for OS media
/// controls. The watch payloads are `Option` because `watch::channel` needs
/// an initial value and there is no meaningful default at startup.
pub struct PlayerSinks {
    pub view_model: watch::Sender<Option<PlayerViewModelLight>>,
    pub queue: watch::Sender<Option<QueueViewModel>>,
    pub media_controls: Option<Arc<dyn MediaControlsSync>>,
}
