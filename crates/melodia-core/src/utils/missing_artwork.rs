//! The producer half of the artwork restore, for the cover caches in `melodia-artwork` and
//! `melodia-views`, which may not name the library. Nothing rides on the event:
//! `tasks::artwork_restore` re-reads the reference set.

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
