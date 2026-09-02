//! Shared section-visibility state for the sidebar sections that cache.
//!
//! Nine views track the same small state machine around "is this section on screen, and
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

/// Generate the three cached-detail accessors the four detail views share.
///
/// Both flips touch the displayed `tracks` cache *and* the canonical `all_tracks` set, or the
/// next `apply_filtered_detail` rebuild drops what was just set.
macro_rules! impl_detail_row_cache {
    ($Ui:ty) => {
        impl $Ui {
            /// Track ids of the **displayed** detail list, in display order — the filtered
            /// subset while a search is active. `play-row` / shuffle / add-to-queue pass these
            /// on, so those act on the visible rows rather than the whole entity.
            pub fn detail_track_ids(&self) -> Vec<i64> {
                self.detail.tracks.lock().iter().map(|r| r.id).collect()
            }

            /// Flip `is_favorite` on the cached detail row, so a single-row toggle needs no
            /// re-fetch.
            pub fn flip_detail_favorite(&self, id: i64, fav: bool) {
                if let Some(r) = self.detail.tracks.lock().iter_mut().find(|r| r.id == id) {
                    r.is_favorite = fav;
                }
                if let Some(r) = self.detail.all_tracks.lock().iter_mut().find(|r| r.id == id) {
                    r.is_favorite = fav;
                }
            }

            /// [`Self::flip_detail_favorite`]'s star-rating twin.
            pub fn flip_detail_rating(&self, id: i64, rating: i32) {
                if let Some(r) = self.detail.tracks.lock().iter_mut().find(|r| r.id == id) {
                    r.rating = rating;
                }
                if let Some(r) = self.detail.all_tracks.lock().iter_mut().find(|r| r.id == id) {
                    r.rating = rating;
                }
            }
        }
    };
}

pub(crate) use {impl_detail_row_cache, impl_section_state_helpers};
