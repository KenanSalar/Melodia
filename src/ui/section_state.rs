//! Shared section-visibility state for the sidebar sections that cache.
//!
//! Six views (`AlbumsUi`, `ArtistsUi`, `GenresUi`, `PlaylistsUi`,
//! `FavoritesUi`, `RecentlyPlayedUi`) track the same small state machine
//! around "is this section on screen, and is its cached data stale?".
//! [`SectionState`] bundles the three fields that machine needs so each `*Ui`
//! carries one cohesive unit instead of three loose, separately-documented
//! fields.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::{Mutex, MutexGuard};

/// Visibility + staleness bookkeeping for one entity-grid section.
///
/// * [`active`](Self::active) — synchronous shadow of "this section is on
///   screen" (mirrors `Nav.selected-index` via the `section-active-changed`
///   callback). Gates background prewarm work so a library-changed tick
///   doesn't re-fill a cache the user isn't looking at.
/// * [`take_dirty`](Self::take_dirty) — sticky "the section was left, so the
///   cached grid / detail data is stale". Set synchronously on section-leave
///   (UI thread, *before* the release task is even spawned) and read-and-
///   cleared on re-enter to choose between a full re-fetch and a cheap
///   cover prewarm. This ordering is what makes it race-correct against an
///   in-flight `release_section_state` wipe.
/// * [`gate`](Self::gate) — serializes a section's bulk-state wipe against
///   the fetch that stores into the same caches (`fetch_grid` on the four
///   entity grids, `favorites::{hero,songs,grids}` and
///   `recently_played::{strip,tracks}` on the two curated pages) so the two
///   can't interleave and leave the visible state inconsistent. Held only
///   around the write/wipe — never across an `.await` (`parking_lot` guard).
pub struct SectionState {
    active: AtomicBool,
    dirty: AtomicBool,
    gate: Mutex<()>,
}

impl SectionState {
    /// A fresh state: not on screen, not dirty. `dirty` starts `false` so a
    /// boot pre-fetch wins the first section-enter without re-fetching.
    ///
    /// **That only holds for a section with nothing shared to publish.** The
    /// four detail sections seed `dirty` themselves when the boot doesn't land
    /// on them (`if !section_active() { mark_dirty() }`, in each
    /// `callbacks/*/lifecycle.rs`), because their pre-fetch runs off-screen and
    /// a hero may only write `HeroBackdrop` / `HeroChips` while it is the one
    /// mounted — so the pre-fetch fills that section's own state but not the
    /// band, and something has to re-fetch once it is visible. Tracks and
    /// Browse take the cheap path as written: neither has a hero.
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

    /// Mark the cached grid + detail data as "must be re-fetched on the
    /// next section-enter". Called synchronously on section-leave.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// Atomically read-and-clear the dirty flag — `true` iff a leave has
    /// marked the data stale since the last call.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    /// Acquire the mutation gate. Hold it around a bulk wipe / data write
    /// so the two can't interleave; drop it before any `.await`.
    pub fn gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock()
    }
}

impl Default for SectionState {
    fn default() -> Self {
        Self::new()
    }
}
