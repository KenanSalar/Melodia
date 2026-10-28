//! A process-wide one-way channel producers reach without carrying a handle, each site keeping
//! its own consumer. The event type is the producer's, so nothing here reaches the consumer's
//! world.

use std::sync::OnceLock;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// The sender half, as a `static`. Unbounded, so no producer can block on a slow consumer.
pub struct EventBridge<E> {
    sender: OnceLock<UnboundedSender<E>>,
}

impl<E> EventBridge<E> {
    pub const fn new() -> Self {
        Self { sender: OnceLock::new() }
    }

    /// `None` once something holds it: the first receiver owns delivery.
    pub fn install(&self) -> Option<UnboundedReceiver<E>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.sender.set(tx).ok().map(|()| rx)
    }

    /// `false` when nothing is installed. A closed channel still reports `true`: a consumer that
    /// shut down isn't one that never existed, and only the second is worth a fallback.
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
