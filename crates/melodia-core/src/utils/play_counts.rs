//! The producer half of the play-count flusher, here so the engine counts a play without naming
//! `tasks`; `tasks::play_count_flusher` is the consumer.

use tokio::sync::mpsc::UnboundedReceiver;

use crate::utils::event_bridge::EventBridge;

#[derive(Debug, Clone, Copy)]
pub enum PlayCountEvent {
    Play(i64),
    Skip(i64),
}

static BRIDGE: EventBridge<PlayCountEvent> = EventBridge::new();

/// `false` before `boot::tasks` installs the flusher; the engine has no other way to write the
/// row.
pub fn try_send(event: PlayCountEvent) -> bool {
    BRIDGE.send(event)
}

/// Claim the bridge for the flusher. `None` once something already holds it.
pub fn install() -> Option<UnboundedReceiver<PlayCountEvent>> {
    BRIDGE.install()
}

#[cfg(test)]
#[path = "tests/play_counts_tests.rs"]
mod tests;
