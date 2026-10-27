use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use melodia_core::utils::block_move;

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
    /// Rows dropped from the play order.
    pub removed: usize,
    /// The current row went, so the caller skips on.
    pub current_was_removed: bool,
}

/// One queued copy of a track, which tells a track queued twice apart from itself. It keeps its
/// value for as long as its row is in the queue, through every move, shuffle and removal around
/// it, and is never handed out again once the row has gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueueEntry(usize);

/// The queue as track ids. The records a transition needs live in the player's `TrackWindow`.
///
/// The three vectors are crate-private because [`Self::play_ids`] caches what they spell, and a
/// write that went round the mutators below would leave it describing an order nobody holds.
#[derive(Debug, Clone)]
pub struct QueueState {
    /// Every entry's track id, each queued entry once, so a track queued twice is two entries.
    pub(crate) tracks: Vec<i64>,
    /// Slots a `clear` has retired, the one write that reuses a slot. A [`QueueEntry`] is its slot
    /// counted past these, so the queue names its entries without storing one per track.
    retired: usize,
    /// Indices into `tracks`.
    pub(crate) play_order: Vec<usize>,
    /// Insertion order, which unshuffling restores.
    pub(crate) original_order: Vec<usize>,
    /// Into `play_order`; `-1` stands for `None` at the disk and Slint boundaries.
    pub current_index: Option<usize>,
    pub shuffle_enabled: bool,
    pub repeat_mode: RepeatMode,
    /// Bumped on every mutation, so `with_state_emit` publishes the queue only when it moved.
    pub version: u64,
    /// `play_order` resolved to ids, built on first ask after the order moves. An advance moves
    /// only the index, so it republishes the queue without rebuilding a library-sized array.
    play_ids: OnceLock<Arc<[i64]>>,
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            retired: 0,
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

/// `-1` for `None`, saturating at `i32::MAX`.
pub fn current_index_to_i32(idx: Option<usize>) -> i32 {
    idx.map_or(-1, |i| i32::try_from(i).unwrap_or(i32::MAX))
}

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

    pub fn len(&self) -> usize {
        self.play_order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.play_order.is_empty()
    }

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

    /// [`Self::play_ids`]' row-for-row twin, built per call: its one reader is the queue panel
    /// while shown, and a cache would sit resident for every install that never opens it.
    pub fn play_entries(&self) -> Arc<[QueueEntry]> {
        self.play_order.iter().map(|&slot| self.entry(slot)).collect()
    }

    /// The entry at `play_index`, for a caller that has to find its row again after an await.
    pub fn entry_at(&self, play_index: usize) -> Option<QueueEntry> {
        self.play_order.get(play_index).map(|&slot| self.entry(slot))
    }

    fn entry(&self, slot: usize) -> QueueEntry {
        QueueEntry(self.retired + slot)
    }

    /// Where `entry` sits in the play order, `None` once its row has left the queue.
    fn play_index_of(&self, entry: QueueEntry) -> Option<usize> {
        self.play_index_of_slot(entry.0.checked_sub(self.retired)?)
    }

    fn play_index_of_slot(&self, slot: usize) -> Option<usize> {
        self.play_order.iter().position(|&queued| queued == slot)
    }

    /// The current row's slot in `tracks`, which is how a reorder finds that row again: by id, a
    /// track queued twice would hand current to whichever copy comes first.
    fn current_slot(&self) -> Option<usize> {
        self.current_index.and_then(|ci| self.play_order.get(ci).copied())
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

    /// Queue `ids` as one block right after the current track, or at the front with nothing
    /// current.
    pub fn insert_next(&mut self, ids: &[i64]) {
        self.insert_at(self.current_index.map_or(0, |ci| ci + 1), ids);
    }

    /// Queues `ids` as one block ahead of the row `before`, named by entry because a caller that
    /// awaited may find the slots shifted. A row that has left since, or `None`, means the end.
    pub fn insert_before(&mut self, before: Option<QueueEntry>, ids: &[i64]) {
        let slot =
            before.and_then(|entry| self.play_index_of(entry)).unwrap_or(self.play_order.len());
        self.insert_at(slot, ids);
    }

    /// Splices `ids` in at `slot`. The insertion order takes the block at its end, so unshuffling
    /// puts it back in the order it was handed over.
    fn insert_at(&mut self, slot: usize, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let slot = slot.min(self.play_order.len());
        let start = self.tracks.len();
        self.tracks.extend_from_slice(ids);
        let block = start..start + ids.len();
        self.original_order.extend(block.clone());
        self.play_order.splice(slot..slot, block);

        if let Some(ci) = self.current_index
            && slot <= ci
        {
            self.current_index = Some(ci + ids.len());
        }
        self.order_changed();
    }

    pub fn remove_at(&mut self, index: usize) -> bool {
        if index >= self.play_order.len() {
            return false;
        }

        let removed_track_idx = self.play_order.remove(index);

        if let Some(pos) = self.original_order.iter().position(|&i| i == removed_track_idx) {
            self.original_order.remove(pos);
        }

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

        // `tracks` keeps the slot so every other index stays valid; `clear` drops it.
        self.order_changed();
        true
    }

    /// Remove multiple play-order indices at once in O(n) time.
    pub fn remove_batch(&mut self, indices: &[usize]) {
        if indices.is_empty() {
            return;
        }

        let index_set: std::collections::HashSet<usize> = indices.iter().copied().collect();

        let removed_track_indices: std::collections::HashSet<usize> =
            index_set.iter().filter_map(|&i| self.play_order.get(i).copied()).collect();

        let (current_removed, removed_before) = match self.current_index {
            Some(ci) => (index_set.contains(&ci), index_set.iter().filter(|&&i| i < ci).count()),
            None => (false, 0),
        };

        let mut pos = 0usize;
        self.play_order.retain(|_| {
            let keep = !index_set.contains(&pos);
            pos += 1;
            keep
        });
        self.original_order.retain(|ti| !removed_track_indices.contains(ti));

        if self.play_order.is_empty() {
            self.current_index = None;
        } else if let Some(ci) = self.current_index {
            let mut next = ci.saturating_sub(removed_before);
            if current_removed {
                next = next.min(self.play_order.len() - 1);
            }
            self.current_index = Some(next);
        }

        self.order_changed();
    }

    /// Moves the play-order slots `rows` together to `to` ([`block_move::move_block`]), the
    /// current track staying current wherever the block lands.
    pub fn move_rows(&mut self, rows: &[usize], to: usize) -> bool {
        let current = self.current_slot();
        if !block_move::move_block(&mut self.play_order, rows, to) {
            return false;
        }
        if let Some(slot) = current {
            self.current_index = self.play_index_of_slot(slot);
        }
        self.order_changed();
        true
    }

    pub fn clear(&mut self) {
        self.retired += self.tracks.len();
        self.tracks.clear();
        self.play_order.clear();
        self.original_order.clear();
        self.current_index = None;
        self.order_changed();
    }

    pub fn skip_to_index(&mut self, index: usize) -> Option<i64> {
        if index >= self.len() {
            return None;
        }
        self.current_index = Some(index);
        self.version += 1;
        self.id_at(index)
    }

    /// Jump to the row `entry` wherever the queue has moved it, `None` once it has left.
    pub fn skip_to_entry(&mut self, entry: QueueEntry) -> Option<i64> {
        let index = self.play_index_of(entry)?;
        self.skip_to_index(index)
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

    /// A manual skip, where `RepeatMode::One` wraps like `All` and only `Off` stops at the end.
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

    /// `indices` permutes positions in `play_order`, not in `tracks`, the current track first.
    pub fn apply_shuffle_order(&mut self, indices: &[usize]) {
        let new_play_order: Vec<usize> =
            indices.iter().filter_map(|&i| self.play_order.get(i).copied()).collect();
        self.play_order = new_play_order;
        self.current_index = Some(0);
        self.shuffle_enabled = true;
        self.order_changed();
    }

    /// [`Self::apply_shuffle_order`] in place with the caller's RNG, sparing both index vectors.
    /// `anchor_to_current` swaps the current track to the front so playback carries on from it.
    pub fn shuffle_play_order_in_place<R: rand::Rng + ?Sized>(
        &mut self,
        rng: &mut R,
        anchor_to_current: bool,
    ) {
        use rand::seq::SliceRandom;
        if self.play_order.is_empty() {
            return;
        }
        let pinned = if anchor_to_current { self.current_slot() } else { None };
        self.play_order.shuffle(rng);
        if let Some(new_pos) = pinned.and_then(|slot| self.play_index_of_slot(slot)) {
            self.play_order.swap(0, new_pos);
        }
        self.current_index = Some(0);
        self.shuffle_enabled = true;
        self.order_changed();
    }

    pub fn unshuffle(&mut self) {
        let current = self.current_slot();
        self.play_order.clone_from(&self.original_order);

        if let Some(pos) = current.and_then(|slot| self.play_index_of_slot(slot)) {
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

    /// Drops the rows whose track is in `ids_to_remove`, their slots staying in `tracks` so every
    /// survivor keeps its [`QueueEntry`]. The current index lands on the next survivor, else the
    /// last. Bumps `version` only when a row went.
    pub fn prune_missing(&mut self, ids_to_remove: &HashSet<i64>) -> PruneOutcome {
        let tracks = &self.tracks;
        let gone = |slot: usize| tracks.get(slot).is_some_and(|id| ids_to_remove.contains(id));

        let removed = self.play_order.iter().filter(|&&slot| gone(slot)).count();
        if removed == 0 {
            return PruneOutcome::default();
        }

        let current_was_removed = self.current_slot().is_some_and(gone);
        // The first survivor at or after the old slot takes the index of the survivors before it.
        let survivors_before_current = self
            .current_index
            .map(|ci| self.play_order.iter().take(ci).filter(|&&slot| !gone(slot)).count());

        self.play_order.retain(|&slot| !gone(slot));
        self.original_order.retain(|&slot| !gone(slot));

        let last = self.play_order.len().checked_sub(1);
        self.current_index =
            survivors_before_current.and_then(|before| last.map(|last| before.min(last)));
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

#[cfg(test)]
#[path = "tests/queue_entry_tests.rs"]
mod entry_tests;
