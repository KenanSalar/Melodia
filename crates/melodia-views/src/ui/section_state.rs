//! Shared section-visibility state for the sidebar sections that cache.
//!
//! Nine views track the same small state machine around "is this section on screen, and
//! is its cached data stale?". [`SectionState`] bundles the fields it needs, so each `*Ui`
//! carries one cohesive unit rather than loose ones.

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
/// * [`grid_on_screen`](Self::grid_on_screen) narrows `active` to the section's card grid,
///   which a detail can cover while the section stays up. `restoring` answers the window no
///   detail id can: a launch reopening the last detail writes the id only once its rows are in.
/// * [`take_deferred_restore`](Self::take_deferred_restore) holds that reopen for a section the
///   launch left hidden, until its first enter.
/// * [`take_grid_owed`](Self::take_grid_owed) is `dirty` for a grid a detail covers while the
///   section stays up, settled by the close rather than by an enter.
pub struct SectionState {
    active: AtomicBool,
    dirty: AtomicBool,
    grid_owed: AtomicBool,
    restoring: AtomicBool,
    deferred_restore: Mutex<Option<i64>>,
    gate: Mutex<()>,
}

impl SectionState {
    /// A fresh state: not on screen, not dirty. `dirty` starts `false` so a boot pre-fetch
    /// wins the first section-enter without re-fetching.
    ///
    /// **That only holds for a section whose pre-fetch fills everything it needs.** Browse
    /// seeds `dirty` itself when the boot doesn't land on it, for the card view's cover tier,
    /// which an off-screen prewarm releases rather than keeps. Every My Library tab seeds it
    /// too when the launch leaves it hidden, the boot skipping its pre-fetch.
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            grid_owed: AtomicBool::new(false),
            restoring: AtomicBool::new(false),
            deferred_restore: Mutex::new(None),
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

    /// Raised beside the detail's own `restoring`, before the reopen of this section's last
    /// detail is spawned, at launch or on a hidden tab's first enter.
    pub fn begin_restore(&self) {
        self.restoring.store(true, Ordering::Release);
    }

    /// Lowered once that reopen returns, however it went.
    pub fn end_restore(&self) {
        self.restoring.store(false, Ordering::Release);
    }

    /// Whether the section's card grid is what is drawn: the section is on screen and no detail
    /// is open over the grid or being reopened over it.
    ///
    /// What a cover prewarm asks rather than [`Self::active`], since the tier keeps what it
    /// decodes until a card draws it again and a grid under a detail draws nothing.
    /// `detail_open` is asked after the flag, which a reopen lowers only once its id is written,
    /// so the two can't both read clear mid-restore.
    pub fn grid_on_screen(&self, detail_open: impl FnOnce() -> bool) -> bool {
        self.active() && !self.restoring.load(Ordering::Acquire) && !detail_open()
    }

    /// Keeps the launch's reopen of `id` for the first enter, the section being hidden.
    ///
    /// A hidden detail can't publish the band it belongs under, so reopening it at launch
    /// decodes its hero and covers for nothing on screen.
    pub fn defer_restore(&self, id: i64) {
        *self.deferred_restore.lock() = Some(id);
    }

    /// The deferred reopen, handed out once. Dropped if a detail is open by then, a drill
    /// that landed first being the newer intent.
    pub fn take_deferred_restore(&self, detail_open: impl FnOnce() -> bool) -> Option<i64> {
        let id = self.deferred_restore.lock().take()?;
        (!detail_open()).then_some(id)
    }

    /// Mark the cached grid and detail data as needing a re-fetch on the next enter.
    /// Called synchronously on section-leave. The enter's reload is the one an owed grid was
    /// waiting for, so it settles that too.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
        self.grid_owed.store(false, Ordering::Release);
    }

    /// Read-and-clear — `true` iff a leave marked the data stale since the last call.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    /// A library change landed while a detail covered the grid. The grid draws nothing under it,
    /// so it reloads once on the close rather than once per change.
    pub fn owe_grid(&self) {
        self.grid_owed.store(true, Ordering::Release);
    }

    /// Read-and-clear, asked by a close that lands back on the grid.
    pub fn take_grid_owed(&self) -> bool {
        self.grid_owed.swap(false, Ordering::AcqRel)
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

/// Generate a view handle's delegating accessors over its `section: SectionState` field.
///
/// The four bodies are one line each and the argument for every one of them is on
/// [`SectionState`] itself, so eight views were carrying eight restatements of it — three of
/// which had already degraded to `/// See [the albums one]`.
macro_rules! impl_section_state_helpers {
    ($Ui:ty) => {
        impl $Ui {
            /// Mirror the section-visible flag, off this view's `section-active-changed`.
            pub fn set_section_active(&self, active: bool) {
                self.section.set_active(active);
            }

            /// Whether this section is currently on screen.
            pub fn section_active(&self) -> bool {
                self.section.active()
            }

            /// Mark the cached data stale. See [`SectionState::mark_dirty`] for why this is
            /// written before the release task rather than by it.
            pub fn mark_dirty(&self) {
                self.section.mark_dirty();
            }

            /// Read-and-clear, deciding whether a section enter re-fetches.
            /// See [`SectionState::take_dirty`].
            pub fn take_dirty(&self) -> bool {
                self.section.take_dirty()
            }
        }
    };
}

pub(crate) use impl_section_state_helpers;

#[cfg(test)]
#[path = "tests/section_state_tests.rs"]
mod tests;
