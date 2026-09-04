//! Ordering for the `views.json` index writes that can fire more than once a tick.
//!
//! Six indices are written this way — the sidebar's `last_nav_index` and the five pages'
//! tab indices — and every one of them is a *catch-up* write: the Slint property is
//! already correct, so nothing on screen depends on the disk hop landing. What does
//! depend on it is the next launch, and there the only value that matters is the last
//! one the UI published.

use std::sync::atomic::{AtomicI32, Ordering};

/// Shadow of a persisted `views.json` index, plus the lock its writes serialize on.
///
/// Two `spawn_blocking` tasks have no ordering of their own, so a burst can land its
/// values reversed and the next launch restores somewhere the user only passed through.
/// A burst is not hypothetical: `nav_history::replay` closes the departing detail before
/// landing the target, and My Library's tab moves on the user's behalf beside a pick.
///
/// [`Self::writer`] supplies the ordering and [`Self::latest`] lets a task holding it drop
/// a superseded write. **The load has to sit under `writer`**, or both tasks pass the
/// check and race for whichever lock the write itself takes.
///
/// Two fields rather than a `Mutex<i32>`, so the UI thread publishes with a store instead
/// of blocking on a guard held across file I/O.
pub(in crate::ui) struct IndexPersist {
    latest: AtomicI32,
    writer: parking_lot::Mutex<()>,
}

impl IndexPersist {
    /// Seeded from the property the writes catch up to rather than from zero. Nothing reads
    /// the shadow before the first publish, but a zero seed would name index 0 as one.
    pub(in crate::ui) fn new(seed: i32) -> Self {
        Self {
            latest: AtomicI32::new(seed),
            writer: parking_lot::Mutex::new(()),
        }
    }

    /// Publish the value about to be written. **UI thread, ahead of the spawn** — a queued
    /// write has to be able to see it.
    pub(in crate::ui) fn publish(&self, idx: i32) {
        self.latest.store(idx, Ordering::Release);
    }

    /// Run `write` unless a later value has been published since. Called from the blocking
    /// pool; `write` runs under the lock, which is what makes the drop sound.
    pub(in crate::ui) fn write_if_current(&self, idx: i32, write: impl FnOnce()) {
        let _writer = self.writer.lock();
        if self.latest.load(Ordering::Acquire) != idx {
            return;
        }
        write();
    }
}
