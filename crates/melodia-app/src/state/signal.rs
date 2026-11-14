//! The three shared cells [`super::AppState`] hands out by the handful, so no field hand-rolls one.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::watch;

/// A settings toggle mirrored where a worker can read it without touching the disk.
///
/// The UI writes the mirror **synchronously before** spawning the persist, so a task that fires in
/// the gap sees the new value. `Relaxed`, nothing being published alongside it.
#[derive(Clone)]
pub struct SharedFlag(Arc<AtomicBool>);

impl SharedFlag {
    pub fn new(value: bool) -> Self {
        Self(Arc::new(AtomicBool::new(value)))
    }

    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set(&self, value: bool) {
        self.0.store(value, Ordering::Relaxed);
    }
}

/// A "something changed, go and look" broadcast, with no payload but a counter.
///
/// A `watch` rather than an `mpsc` because only the latest matters: a burst collapses into one
/// wake-up. The UI subscribes through `ui::signal::on_signal`. Wrapping, because the count means
/// nothing but that it moved.
#[derive(Clone)]
pub struct Signal(watch::Sender<u64>);

impl Signal {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(0);
        Self(tx)
    }

    pub fn bump(&self) {
        self.0.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}

impl Default for Signal {
    fn default() -> Self {
        Self::new()
    }
}

/// How many of something are under way, each counted for as long as its [`Hold`] lives. A count
/// rather than a flag, so two that overlap can't clear each other.
#[derive(Clone)]
pub struct HoldCount(watch::Sender<u32>);

impl HoldCount {
    pub fn new() -> Self {
        Self(watch::Sender::new(0))
    }

    pub fn hold(&self) -> Hold {
        self.0.send_modify(|count| *count += 1);
        Hold(self.0.clone())
    }

    pub fn is_held(&self) -> bool {
        *self.0.borrow() > 0
    }

    pub fn subscribe(&self) -> watch::Receiver<u32> {
        self.0.subscribe()
    }
}

impl Default for HoldCount {
    fn default() -> Self {
        Self::new()
    }
}

/// One count on a [`HoldCount`], released on drop, so work that stops or fails part way can't
/// leave it raised.
pub struct Hold(watch::Sender<u32>);

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count = count.saturating_sub(1));
    }
}

#[cfg(test)]
#[path = "tests/signal_tests.rs"]
mod tests;
