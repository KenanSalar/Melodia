//! The producer half of the play-count flusher: what the engine says, and how it says it.
//!
//! Here rather than beside the flusher because `player::actions` is the only thing that sends,
//! and the flusher is a `tasks/` job that owns a `DbPool` — an engine naming that directly is
//! the edge the whole layering exists to forbid. What crosses is an enum and a `send`.
//!
//! [`crate::tasks::play_count_flusher`] is the consumer, and argues the batching.

use tokio::sync::mpsc::UnboundedReceiver;

use crate::utils::event_bridge::EventBridge;

/// Event emitted by `PlayerAction::UpdatePlayCount` / `UpdateSkipCount`.
#[derive(Debug, Clone, Copy)]
pub enum PlayCountEvent {
    Play(i64),
    Skip(i64),
}

/// Process-wide sender. Claimed at startup by the flusher's `spawn`; read by `execute_actions`
/// so we don't have to thread an extra parameter through 30 call sites.
static BRIDGE: EventBridge<PlayCountEvent> = EventBridge::new();

/// Send an event to the flusher. `false` when nothing has been installed, which outside a test
/// means only the window before `boot::tasks` runs — the engine has no other way to write the
/// row and does not want one.
pub fn try_send(event: PlayCountEvent) -> bool {
    BRIDGE.send(event)
}

/// Claim the bridge for the flusher. `None` once something already holds it.
pub fn install() -> Option<UnboundedReceiver<PlayCountEvent>> {
    BRIDGE.install()
}
