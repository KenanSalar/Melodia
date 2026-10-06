//! The producer half of the artwork restore: a cover cache saying a stored file it was asked for
//! is gone.
//!
//! Here because the caches that notice sit in `melodia-artwork` and `melodia-views`, and what
//! answers is a library scan neither may name. What crosses is a bare `report`; the consumer
//! re-reads the reference set rather than trusting which path was asked for, so nothing rides on
//! the event.
//!
//! `tasks::artwork_restore` is the consumer.

use tokio::sync::mpsc::UnboundedReceiver;

use crate::utils::event_bridge::EventBridge;

static BRIDGE: EventBridge<()> = EventBridge::new();

/// Says a stored cover a cache was asked for is no longer on disk. No-op before the consumer is
/// installed, the boot scan restoring anything missing by then on its own.
pub fn report() {
    BRIDGE.send(());
}

/// Claim the bridge for the restore task. `None` once something already holds it.
pub fn install() -> Option<UnboundedReceiver<()>> {
    BRIDGE.install()
}
