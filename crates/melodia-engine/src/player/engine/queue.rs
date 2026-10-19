use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use super::types::{PersistableQueue, RepeatMode};

/// Slots before the current one whose records the window holds: the one Previous lands on.
const WINDOW_BEHIND: i64 = 1;

/// Slots after the current one whose records the window holds. The first is next under any repeat
/// mode and under a manual skip; the two beyond carry an auto-skip past unplayable files without a
/// fetch.
const WINDOW_AHEAD: i64 = 3;

/// Result of [`QueueState::prune_missing`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneOutcome {
    /// Number of distinct entries removed from `tracks`.
    pub removed: usize,
    /// True if the `play_order[current_index]` entry — the thing the player
    /// believes is currently playing — was in `ids_to_remove`. Caller uses
    /// this to decide whether to auto-skip to the next track.
    pub current_was_removed: bool,
}

/// The queue as track ids. The records a transition needs live in the player's `TrackWindow`.
///
/// The three vectors are crate-private because [`Self::play_ids`] caches what they spell, and a
/// write that went round the mutators below would leave it describing an order nobody holds.
#[derive(Debug, Clone)]
pub struct QueueState {
    /// Every entry's track id, each queued entry once, so a track queued twice is two entries.
    pub(crate) tracks: Vec<i64>,
    /// Current playback order — indices into `tracks`.
    pub(crate) play_order: Vec<usize>,
    /// Original insertion order — indices into `tracks`. Used to restore on unshuffle.
    pub(crate) original_order: Vec<usize>,
    /// Index into `play_order` of the currently-playing track, or `None`
    /// when nothing is playing. The on-disk and Slint representations use a
    /// signed integer with `-1` as the no-track sentinel; conversion happens
    /// at those boundaries (`to_persistable`, `to_view_model`).
    pub current_index: Option<usize>,
    pub shuffle_enabled: bool,
    pub repeat_mode: RepeatMode,
    /// Monotonic counter incremented on every queue mutation.
    /// Used by `with_state_emit` to detect whether the queue changed.
    pub version: u64,
    /// `play_order` resolved to ids, built on first ask after the order moves. An advance moves
    /// only the index, so it republishes the queue without rebuilding a library-sized array.
    play_ids: OnceLock<Arc<[i64]>>,
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            play_order: Vec::new(),
            original_order: Vec::new(),
            current_index: None,
            shuffle_enabled: false,
            repeat_mode: RepeatMode::Off,
            version: 0,
            play_ids: OnceLock::new(),
        }
    }
}

/// Encode `current_index` as the on-disk / Slint i32 representation
/// (`-1` for `None`, otherwise the index saturated to `i32::MAX`).
pub fn current_index_to_i32(idx: Option<usize>) -> i32 {
    idx.map_or(-1, |i| i32::try_from(i).unwrap_or(i32::MAX))
}

/// Decode an i32 (`-1` sentinel, otherwise a non-negative index) into the
/// internal `Option<usize>` form.
pub fn current_index_from_i32(idx: i32) -> Option<usize> {
    usize::try_from(idx).ok()
}

/// The play-order slots of a queue of `len` whose records the window holds around `current`: the
/// one before it, it and the three after, wrapping where `wraps` as `previous` and `advance_skip`
/// do. With nothing current the next slot is the first.
pub fn window_slots(
    len: usize,
    current: Option<usize>,
    wraps: bool,
) -> impl Iterator<Item = usize> {
    let span = i64::try_from(len).unwrap_or(0);
    let current = current.and_then(|ci| i64::try_from(ci).ok()).unwrap_or(-1);
    (-WINDOW_BEHIND..=WINDOW_AHEAD).filter_map(move |offset| {
        if span == 0 {
            return None;
        }
        let slot = current + offset;
        let slot = if wraps { slot.rem_euclid(span) } else { slot };
        usize::try_from(slot).ok().filter(|&slot| slot < len)
    })
}

impl QueueState {
    /// A queue of `ids` in their own order, as `queue.json` saved it. An index past the end names
    /// no track, so it seats none.
    pub fn restored(ids: Vec<i64>, current_index: Option<usize>) -> Self {
        let len = ids.len();
        Self {
            tracks: ids,
            play_order: (0..len).collect(),
            original_order: (0..len).collect(),
            current_index: current_index.filter(|&ci| ci < len),
            ..Self::default()
        }
    }

    /// Number of tracks in the current play order.
    pub fn len(&self) -> usize {
        self.play_order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.play_order.is_empty()
    }

    /// The track at an index in `play_order`.
    pub fn id_at(&self, play_index: usize) -> Option<i64> {
        self.play_order.get(play_index).and_then(|&ti| self.tracks.get(ti).copied())
    }

    /// Every track in play order, shared with whoever holds the last copy until the order moves.
    pub fn play_ids(&self) -> Arc<[i64]> {
        let ids = self.play_ids.get_or_init(|| {
            self.play_order.iter().filter_map(|&ti| self.tracks.get(ti).copied()).collect()
        });
        Arc::clone(ids)
    }

    /// The tracks a transition can move onto without a fetch, those at [`window_slots`], each id
    /// once.
    pub fn window_ids(&self) -> Vec<i64> {
        let mut ids = Vec::new();
        for slot in window_slots(self.len(), self.current_index, self.repeat_mode.wraps()) {
            if let Some(id) = self.id_at(slot)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        ids
    }

    /// Forget the cached ids and count a mutation, for every change that moves the order rather
    /// than just the index.
    fn order_changed(&mut self) {
        self.play_ids = OnceLock::new();
        self.version += 1;
    }

    pub fn add_tracks(&mut self, ids: &[i64]) {
        let start = self.tracks.len();
        self.tracks.extend_from_slice(ids);
        let range = start..start + ids.len();
        self.play_order.extend(range.clone());
        self.original_order.extend(range);
        self.order_changed();
    }

    pub fn insert_next(&mut self, id: i64) {
        let track_idx = self.tracks.len();
        self.tracks.push(id);
        self.original_order.push(track_idx);

        let insert_pos = self.current_index.map_or(0, |ci| ci + 1).min(self.play_order.len());
        self.play_order.insert(insert_pos, track_idx);
        self.order_changed();
    }

    pub fn remove_at(&mut self, index: usize) -> bool {
        if index >= self.play_order.len() {
            return false;
        }

        let removed_track_idx = self.play_order.remove(index);

        // Remove from original_order
        if let Some(pos) = self.original_order.iter().position(|&i| i == removed_track_idx) {
            self.original_order.remove(pos);
        }

        // Adjust current_index
        if let Some(ci) = self.current_index {
            if index < ci {
                self.current_index = Some(ci - 1);
            } else if index == ci {
                if self.play_order.is_empty() {
                    self.current_index = None;
                } else if ci >= self.play_order.len() {
                    self.current_index = Some(self.play_order.len() - 1);
                }
            }
        }

        // Note: we don't remove from self.tracks to avoid invalidating all indices.
        // Orphaned track data is cleaned up on queue clear.
        self.order_changed();
        true
    }

    /// Remove multiple play-order indices at once in O(n) time.
    pub fn remove_batch(&mut self, indices: &[usize]) {
        if indices.is_empty() {
            return;
        }

        let index_set: std::collections::HashSet<usize> = indices.iter().copied().collect();

        // Collect the track indices being removed (for original_order cleanup).
        let removed_track_indices: std::collections::HashSet<usize> =
            index_set.iter().filter_map(|&i| self.play_order.get(i).copied()).collect();

        // Determine whether the currently-playing slot is being removed and
        // count how many removed indices fall before it.
        let (current_removed, removed_before) = match self.current_index {
            Some(ci) => (index_set.contains(&ci), index_set.iter().filter(|&&i| i < ci).count()),
            None => (false, 0),
        };

        // Retain only non-removed entries in play_order and original_order.
        let mut pos = 0usize;
        self.play_order.retain(|_| {
            let keep = !index_set.contains(&pos);
            pos += 1;
            keep
        });
        self.original_order.retain(|ti| !removed_track_indices.contains(ti));

        // Adjust current_index.
        if self.play_order.is_empty() {
            self.current_index = None;
        } else if let Some(ci) = self.current_index {
            let mut next = ci.saturating_sub(removed_before);
            if current_removed {
                // Clamp to valid range after the track at current_index was removed.
                next = next.min(self.play_order.len() - 1);
            }
            self.current_index = Some(next);
        }

        self.order_changed();
    }

    pub fn move_track(&mut self, from: usize, to: usize) -> bool {
        if from >= self.play_order.len() || to >= self.play_order.len() {
            return false;
        }

        let idx = self.play_order.remove(from);
        self.play_order.insert(to, idx);

        if let Some(ci) = self.current_index {
            self.current_index = Some(if from == ci {
                to
            } else if from < ci && to >= ci {
                ci - 1
            } else if from > ci && to <= ci {
                ci + 1
            } else {
                ci
            });
        }
        self.order_changed();
        true
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.play_order.clear();
        self.original_order.clear();
        self.current_index = None;
        self.order_changed();
    }

    /// Jump to a specific index in the play order.
    pub fn skip_to_index(&mut self, index: usize) -> Option<i64> {
        if index >= self.len() {
            return None;
        }
        self.current_index = Some(index);
        self.version += 1;
        self.id_at(index)
    }

    pub fn current_id(&self) -> Option<i64> {
        self.id_at(self.current_index?)
    }

    pub fn advance(&mut self) -> Option<i64> {
        if self.play_order.is_empty() {
            return None;
        }

        self.version += 1;
        let len = self.len();
        let next_from = |ci: Option<usize>| ci.map_or(0, |i| i + 1);
        match self.repeat_mode {
            RepeatMode::One => self.current_id(),
            RepeatMode::All => {
                let next = next_from(self.current_index) % len;
                self.current_index = Some(next);
                self.current_id()
            }
            RepeatMode::Off => {
                let next_index = next_from(self.current_index);
                if next_index >= len {
                    return None;
                }
                self.current_index = Some(next_index);
                self.current_id()
            }
        }
    }

    /// User-initiated skip — advances to next track.
    /// `RepeatMode::One` wraps like `RepeatMode::All`: manual navigation
    /// walks the queue in a loop. Only `Off` stops at the end.
    pub fn advance_skip(&mut self) -> Option<i64> {
        if self.play_order.is_empty() {
            return None;
        }

        self.version += 1;
        let len = self.len();
        let next_from = |ci: Option<usize>| ci.map_or(0, |i| i + 1);
        match self.repeat_mode {
            RepeatMode::All | RepeatMode::One => {
                let next = next_from(self.current_index) % len;
                self.current_index = Some(next);
                self.current_id()
            }
            RepeatMode::Off => {
                let next_index = next_from(self.current_index);
                if next_index >= len {
                    return None;
                }
                self.current_index = Some(next_index);
                self.current_id()
            }
        }
    }

    pub fn previous(&mut self) -> Option<i64> {
        if self.play_order.is_empty() {
            return None;
        }

        self.version += 1;
        if self.repeat_mode.wraps() {
            // Wraps. None and Some(0) both go to the last track.
            let next = self.current_index.filter(|&ci| ci > 0).map_or(self.len() - 1, |ci| ci - 1);
            self.current_index = Some(next);
        } else if let Some(ci) = self.current_index
            && ci > 0
        {
            self.current_index = Some(ci - 1);
        }
        // Off with None or Some(0): no change.
        self.current_id()
    }

    pub fn peek_next(&self) -> Option<i64> {
        if self.play_order.is_empty() {
            return None;
        }

        let len = self.len();
        let next_from = |ci: Option<usize>| ci.map_or(0, |i| i + 1);
        match self.repeat_mode {
            RepeatMode::One => self.current_id(),
            RepeatMode::All => self.id_at(next_from(self.current_index) % len),
            RepeatMode::Off => {
                let next = next_from(self.current_index);
                if next >= len { None } else { self.id_at(next) }
            }
        }
    }

    /// Begin shuffle: returns (`track_count`, `current_track_index`) for the shell to generate random indices.
    pub fn begin_shuffle(&self) -> (usize, Option<usize>) {
        (self.play_order.len(), self.current_index)
    }

    /// Apply a shuffled order provided by the shell.
    /// `indices` is a permutation of `0..play_order.len()` with the current track at index 0.
    /// These are indices into `play_order`, not into `tracks`.
    pub fn apply_shuffle_order(&mut self, indices: &[usize]) {
        // Remap: new play_order[i] = old play_order[indices[i]]
        let new_play_order: Vec<usize> =
            indices.iter().filter_map(|&i| self.play_order.get(i).copied()).collect();
        self.play_order = new_play_order;
        self.current_index = Some(0);
        self.shuffle_enabled = true;
        self.order_changed();
    }

    /// In-place version of [`Self::apply_shuffle_order`] that shuffles
    /// `play_order` directly using the caller-provided RNG. Avoids the
    /// double `Vec<usize>` allocation that the shell-driven path required
    /// (one for the indices in the caller, one for the remapped play
    /// order here). If `anchor_to_current` is true and the queue has a
    /// current track, that track is swapped to the front of the shuffled
    /// order so playback carries on from it.
    pub fn shuffle_play_order_in_place<R: rand::Rng + ?Sized>(
        &mut self,
        rng: &mut R,
        anchor_to_current: bool,
    ) {
        use rand::seq::SliceRandom;
        if self.play_order.is_empty() {
            return;
        }
        let pinned_track_idx = if anchor_to_current {
            self.current_index.and_then(|ci| self.play_order.get(ci).copied())
        } else {
            None
        };
        self.play_order.shuffle(rng);
        if let Some(ti) = pinned_track_idx
            && let Some(new_pos) = self.play_order.iter().position(|&i| i == ti)
        {
            self.play_order.swap(0, new_pos);
        }
        self.current_index = Some(0);
        self.shuffle_enabled = true;
        self.order_changed();
    }

    /// Restore original track order (unshuffle).
    pub fn unshuffle(&mut self) {
        let current_track_id = self.current_id();
        self.play_order.clone_from(&self.original_order);

        if let Some(track_id) = current_track_id
            && let Some(pos) =
                self.play_order.iter().position(|&ti| self.tracks.get(ti) == Some(&track_id))
        {
            self.current_index = Some(pos);
        }
        self.shuffle_enabled = false;
        self.order_changed();
    }

    pub fn cycle_repeat_mode(&mut self) {
        self.set_repeat_mode(match self.repeat_mode {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        });
    }

    /// Bumps `version` only on a move, so a panel re-sending the mode already set re-publishes
    /// no queue.
    pub fn set_repeat_mode(&mut self, mode: RepeatMode) {
        if self.repeat_mode == mode {
            return;
        }
        self.repeat_mode = mode;
        self.version += 1;
    }

    /// Drop tracks whose ID is in `ids_to_remove` from the queue. Maintains the
    /// `tracks` / `play_order` / `original_order` invariants by remapping all
    /// surviving track-indices. Adjusts `current_index` to land on the next
    /// surviving play-order entry at or after the old slot (or, if everything
    /// at or after was removed, the last survivor — or `None` for an empty
    /// queue).
    ///
    /// Bumps `version` only when at least one entry actually changed, so a
    /// no-op call (empty input or no matches) won't trigger a `ViewModel` re-emit.
    pub fn prune_missing(&mut self, ids_to_remove: &HashSet<i64>) -> PruneOutcome {
        if ids_to_remove.is_empty() || self.tracks.is_empty() {
            return PruneOutcome::default();
        }

        // Step 1: classify each `tracks` slot and build the old→new remap.
        let old_len = self.tracks.len();
        let mut keep_mask: Vec<bool> = Vec::with_capacity(old_len);
        let mut remap: Vec<Option<usize>> = Vec::with_capacity(old_len);
        let mut new_tracks: Vec<i64> = Vec::with_capacity(old_len);
        for &id in &self.tracks {
            if ids_to_remove.contains(&id) {
                keep_mask.push(false);
                remap.push(None);
            } else {
                keep_mask.push(true);
                remap.push(Some(new_tracks.len()));
                new_tracks.push(id);
            }
        }

        let removed = old_len - new_tracks.len();

        if removed == 0 {
            return PruneOutcome::default();
        }

        // Step 2: figure out whether the "currently playing" entry is among
        // the casualties — *before* mutating anything else, so `current_index`
        // still resolves through the old `play_order`.
        let current_was_removed = self
            .current_index
            .and_then(|ci| self.play_order.get(ci).copied())
            .is_some_and(|ti| !keep_mask[ti]);

        // Step 3: rebuild play_order, dropping entries whose target was
        // removed, and remember where each old slot lands (or that it
        // didn't) so we can fix up `current_index` afterwards.
        let old_play_order = std::mem::take(&mut self.play_order);
        let mut new_play_order: Vec<usize> = Vec::with_capacity(old_play_order.len());
        let mut slot_remap: Vec<Option<usize>> = Vec::with_capacity(old_play_order.len());
        for old_ti in old_play_order {
            match remap[old_ti] {
                Some(new_ti) => {
                    slot_remap.push(Some(new_play_order.len()));
                    new_play_order.push(new_ti);
                }
                None => slot_remap.push(None),
            }
        }
        self.play_order = new_play_order;

        // Step 4: rebuild original_order with the same remap rules. We don't
        // need a slot map for it — it's never indexed by `current_index`.
        let old_original_order = std::mem::take(&mut self.original_order);
        let mut new_original_order: Vec<usize> = Vec::with_capacity(old_original_order.len());
        for old_ti in old_original_order {
            if let Some(new_ti) = remap[old_ti] {
                new_original_order.push(new_ti);
            }
        }
        self.original_order = new_original_order;

        // Step 5: recompute current_index in the new play_order space.
        // Strategy: find the next surviving slot at or after the old index;
        // fall back to the last survivor; fall back to None when empty.
        self.current_index = self.current_index.and_then(|old_ci| {
            slot_remap
                .iter()
                .skip(old_ci)
                .find_map(|s| *s)
                .or_else(|| self.play_order.len().checked_sub(1))
        });

        self.tracks = new_tracks;
        self.order_changed();

        PruneOutcome { removed, current_was_removed }
    }

    pub fn to_persistable(&self) -> PersistableQueue {
        PersistableQueue {
            track_ids: self.play_ids().to_vec(),
            current_index: current_index_to_i32(self.current_index),
        }
    }
}

#[cfg(test)]
#[path = "tests/queue_tests.rs"]
mod tests;
