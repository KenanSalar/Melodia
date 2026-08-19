//! Shared section-visibility state for the sidebar sections that cache.
//!
//! Six views track the same small state machine around "is this section on screen, and
//! is its cached data stale?". [`SectionState`] bundles the three fields it needs, so
//! each `*Ui` carries one cohesive unit rather than three loose ones.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::{Mutex, MutexGuard};

/// Visibility + staleness bookkeeping for one entity-grid section.
///
/// * [`active`](Self::active) — synchronous shadow of "this section is on screen", gating
///   background prewarm so a library-changed tick doesn't re-fill a cache nobody is
///   looking at.
/// * [`take_dirty`](Self::take_dirty) — sticky "the section was left, so its cached data
///   is stale". Set synchronously on leave *before* the release task is spawned, which is
///   what makes it race-correct against an in-flight `release_section_state` wipe.
/// * [`gate`](Self::gate) — serializes that wipe against the fetch storing into the same
///   caches. Held only around the write or wipe, never across an `.await`.
pub struct SectionState {
    active: AtomicBool,
    dirty: AtomicBool,
    gate: Mutex<()>,
}

impl SectionState {
    /// A fresh state: not on screen, not dirty. `dirty` starts `false` so a boot pre-fetch
    /// wins the first section-enter without re-fetching.
    ///
    /// **That only holds for a section whose pre-fetch fills everything it needs.** The
    /// four detail sections seed `dirty` themselves when the boot doesn't land on them
    /// (`if !section_active() { mark_dirty() }`): a hero may only write `HeroBackdrop` /
    /// `HeroChips` while it is the mounted one, so an off-screen pre-fetch fills that
    /// section's own state but not the band. **Browse takes the same seed for the card
    /// view's cover tier**, which an off-screen prewarm releases rather than keeps.
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            gate: Mutex::new(()),
        }
    }

    /// Mirror the section-visible flag (`section-active-changed`).
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
    }

    /// Whether the section is currently on screen.
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Mark the cached grid and detail data as needing a re-fetch on the next enter.
    /// Called synchronously on section-leave.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Read-and-clear — `true` iff a leave marked the data stale since the last call.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    /// Hold around a bulk wipe or data write so the two can't interleave; drop it before
    /// any `.await`.
    pub fn gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock()
    }
}

impl Default for SectionState {
    fn default() -> Self {
        Self::new()
    }
}
