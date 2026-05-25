use std::sync::Arc;

use tokio::sync::watch;

use super::state::{PlayerViewModelLight, QueueViewModel};
use super::types::PlaybackStatus;

/// Events that come *from* OS media controls (souvlaki) into the player.
/// Phase 1 has no real sink wired; Phase 2 wires the Slint-bridged sink.
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
/// `update_position` is the lighter periodic position refresh used by the
/// playback monitor on macOS / Windows. Sub-phase F implements this on the
/// real `MediaControlsHandle`; in Phase 1 nothing is wired (`PlayerSinks`
/// holds `None`) so the trait is dormant.
pub trait MediaControlsSync: Send + Sync + 'static {
    fn sync(&self, vm: &PlayerViewModelLight, status: PlaybackStatus);
    fn update_position(&self, _position_ms: u64) {}
}

/// Sinks consumed by `with_state_emit`. The two watch senders carry the
/// reactive view-model updates that the Slint bridge will subscribe to in
/// Phase 2; `media_controls` is the optional push-direction hook for OS
/// media controls. The watch payloads are wrapped in `Option` because
/// `watch::channel` requires an initial value, and at startup there is no
/// meaningful default — `None` is the cleanest sentinel.
pub struct PlayerSinks {
    pub view_model: watch::Sender<Option<PlayerViewModelLight>>,
    pub queue: watch::Sender<Option<QueueViewModel>>,
    pub media_controls: Option<Arc<dyn MediaControlsSync>>,
}
