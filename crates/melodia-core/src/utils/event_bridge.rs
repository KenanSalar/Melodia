//! A process-wide one-way channel whose sender producers reach without carrying a handle.
//!
//! Two of these exist and they had a copy each of the same three pieces: a `OnceLock` sender, a
//! send that no-ops when nothing installed one, and an install that hands the receiver back. What
//! differs between them is only the consumer — the play-count flusher spawns its own, the toast
//! bridge hands its receiver to a UI-thread task — so that half stays at each site.
//!
//! Producers are dependency-free by construction: the event type is theirs and nothing here
//! reaches the consumer's world. That is what lets `tasks/` and `player/` raise a toast without
//! importing `ui`.

use std::sync::OnceLock;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// The sender half, as a `static`. Unbounded, so no producer can block on a slow consumer.
pub struct EventBridge<E> {
    sender: OnceLock<UnboundedSender<E>>,
}

impl<E> EventBridge<E> {
    pub const fn new() -> Self {
        Self {
            sender: OnceLock::new(),
        }
    }

    /// Claim the bridge and hand back the receiver to drain.
    ///
    /// `None` once something already holds it: the first receiver owns delivery, so a second
    /// install is a no-op rather than a silent hand-over.
    pub fn install(&self) -> Option<UnboundedReceiver<E>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.sender.set(tx).ok().map(|()| rx)
    }

    /// Queue an event from any thread. `false` when nothing is installed.
    ///
    /// A send to a closed channel is dropped and still reports `true` — the consumer having shut
    /// down is not the same as never having existed, and only the second is worth a fallback.
    pub fn send(&self, event: E) -> bool {
        let Some(tx) = self.sender.get() else {
            return false;
        };
        let _ = tx.send(event);
        true
    }
}

impl<E> Default for EventBridge<E> {
    fn default() -> Self {
        Self::new()
    }
}
