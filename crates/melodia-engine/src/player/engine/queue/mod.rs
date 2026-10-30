mod removal;

use std::sync::{Arc, OnceLock};

use melodia_core::utils::block_move;

use super::types::{PersistableQueue, RepeatMode};

pub use removal::{ClearedQueue, PruneOutcome, QueueCut};

/// Slots before the current one whose records the window holds: the one Previous lands on.
const WINDOW_BEHIND: i64 = 1;

/// Slots after the current one whose records the window holds. The first is next under any repeat
/// mode and under a manual skip; the two beyond carry an auto-skip past unplayable files without a
/// fetch.
const WINDOW_AHEAD: i64 = 3;

/// One queued copy of a track, which tells a track queued twice apart from itself. It keeps its
/// value for as long as its row is in the queue, through every move, shuffle and removal around
/// it, and is never handed out again once the row has gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueueEntry(usize);

/// The queue as track ids. The records a transition needs live in the player's `TrackWindow`.
///
/// The three vectors are crate-private because [`Self::play_ids`] caches what they spell, and a
/// write that went round its mutators would leave it describing an order nobody holds.
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
    /// Into `play_order`; `-1` stands for `None` on disk.
    pub current_index: Option<usize>,
    /// The playing track's row has left while the track plays on, so `current_index` names the row
    /// that took its place, not yet started, or `None` past the end. Held only while a track is
    /// seated: `PlayerState::close_gap_unless_seated` and a station's seat close it.
    playing_left: bool,
    /// Bumped by every [`Self::seat`], so a restore can tell whether a row has started since its
    /// removal.
    seats: u64,
    /// Someone placed these rows: an append, a Play Next, a drop-in, a move or a cut. Only then
    /// does [`Self::replace`] hold what it takes.
    hand_edited: bool,
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
            playing_left: false,
            seats: 0,
            hand_edited: false,
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
    /// no track, so it seats none. Where the queue came from isn't saved, so it counts as built by
    /// hand.
    pub fn restored(ids: Vec<i64>, current_index: Option<usize>) -> Self {
        let len = ids.len();
        Self {
            tracks: ids,
            play_order: (0..len).collect(),
            original_order: (0..len).collect(),
            current_index: current_index.filter(|&ci| ci < len),
            hand_edited: true,
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

    /// The entry at `play_index` while its row still holds `track_id`, for a caller drawing from a
    /// publish that can trail the queue.
    pub fn entry_holding(&self, play_index: usize, track_id: i64) -> Option<QueueEntry> {
        let slot = *self.play_order.get(play_index)?;
        (self.tracks.get(slot) == Some(&track_id)).then(|| self.entry(slot))
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

    pub(crate) fn playing_left(&self) -> bool {
        self.playing_left
    }

    /// The row shown as playing, none while the playing track has left the queue.
    pub fn playing_index(&self) -> Option<usize> {
        self.current_index.filter(|_| !self.playing_left)
    }

    /// The first row still to play: the current one while the playing track has left the queue,
    /// else the one after it.
    pub fn upcoming_from(&self) -> usize {
        match self.current_index {
            Some(ci) if self.playing_left => ci,
            Some(ci) => ci + 1,
            None if self.playing_left => self.len(),
            None => 0,
        }
    }

    /// The tracks a transition can move onto without a fetch, those at [`window_slots`], each id
    /// once.
    pub fn window_ids(&self) -> Vec<i64> {
        // Anchored at the gap, so one at the end still holds the row Previous lands on.
        let anchor =
            if self.playing_left { Some(self.upcoming_from()) } else { self.current_index };
        let mut ids = Vec::new();
        for slot in window_slots(self.len(), anchor, self.repeat_mode.wraps()) {
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
        self.insert_at(self.play_order.len(), ids);
    }

    /// Queue `ids` as one block to play next.
    pub fn insert_next(&mut self, ids: &[i64]) {
        self.insert_at(self.upcoming_from(), ids);
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
        let fills_gap = self.playing_left && slot == self.upcoming_from();
        let start = self.tracks.len();
        self.tracks.extend_from_slice(ids);
        let block = start..start + ids.len();
        self.original_order.extend(block.clone());
        self.play_order.splice(slot..slot, block);

        if fills_gap {
            self.current_index = Some(slot);
        } else if let Some(ci) = self.current_index
            && slot <= ci
        {
            self.current_index = Some(ci + ids.len());
        }
        self.hand_edited = true;
        self.order_changed();
    }

    /// Ends the gap a removal left, for a deck with no track seated to play on: the row that took
    /// the playing one's place is current, or past the end the last.
    pub fn close_gap(&mut self) {
        if !std::mem::take(&mut self.playing_left) {
            return;
        }
        if self.current_index.is_none() {
            self.current_index = self.len().checked_sub(1);
        }
        self.version += 1;
    }

    /// Moves the play-order slots `rows` together to `to` ([`block_move::move_block`]), the
    /// current track staying current wherever the block lands. A gap stays between the rows it sat
    /// between, a block dropped into it playing next.
    pub fn move_rows(&mut self, rows: &[usize], to: usize) -> bool {
        let current = self.current_slot();
        let staying_before_gap =
            self.playing_left.then(|| block_move::landing(rows, self.upcoming_from()));
        if !block_move::move_block(&mut self.play_order, rows, to) {
            return false;
        }
        if let Some(before) = staying_before_gap {
            let gap = if to < before { before + rows.len() } else { before };
            self.current_index = (gap < self.len()).then_some(gap);
        } else if let Some(slot) = current {
            self.current_index = self.play_index_of_slot(slot);
        }
        self.hand_edited = true;
        self.order_changed();
        true
    }

    /// Makes `index` the current row, its track the one playing.
    pub fn seat(&mut self, index: usize) {
        self.current_index = Some(index);
        self.playing_left = false;
        self.seats += 1;
    }

    pub fn skip_to_index(&mut self, index: usize) -> Option<i64> {
        if index >= self.len() {
            return None;
        }
        self.seat(index);
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
        if self.repeats_current() {
            return self.current_id();
        }
        let next = self.next_slot(self.repeat_mode == RepeatMode::All)?;
        self.seat(next);
        self.current_id()
    }

    /// A manual skip, where `RepeatMode::One` wraps like `All` and only `Off` stops at the end.
    pub fn advance_skip(&mut self) -> Option<i64> {
        if self.play_order.is_empty() {
            return None;
        }

        self.version += 1;
        let next = self.next_slot(self.repeat_mode.wraps())?;
        self.seat(next);
        self.current_id()
    }

    /// Whether Next has a row to step to.
    pub fn has_next(&self) -> bool {
        self.next_slot(self.repeat_mode.wraps()).is_some()
    }

    pub fn peek_next(&self) -> Option<i64> {
        if self.repeats_current() {
            return self.current_id();
        }
        self.next_slot(self.repeat_mode == RepeatMode::All).and_then(|slot| self.id_at(slot))
    }

    /// `RepeatMode::One` plays the playing row again, which a row that took its place isn't.
    fn repeats_current(&self) -> bool {
        self.repeat_mode == RepeatMode::One && !self.playing_left
    }

    /// The row a step forward lands on, the first again where `wraps` past the end.
    fn next_slot(&self, wraps: bool) -> Option<usize> {
        let next = self.upcoming_from();
        if next < self.len() { Some(next) } else { (wraps && !self.is_empty()).then_some(0) }
    }

    pub fn previous(&mut self) -> Option<i64> {
        if self.play_order.is_empty() {
            return None;
        }

        self.version += 1;
        let wrapped = self.repeat_mode.wraps().then(|| self.len() - 1);
        match self.row_before().or(wrapped) {
            Some(slot) => {
                self.seat(slot);
                self.current_id()
            }
            // Off with nothing before restarts what plays, under a gap the seated track rather
            // than the current row.
            None if self.playing_left => None,
            None => self.current_id(),
        }
    }

    /// Whether Previous has a row to step back to.
    pub fn has_previous(&self) -> bool {
        !self.is_empty() && (self.repeat_mode.wraps() || self.row_before().is_some())
    }

    /// The row before the playing one, or before the gap it left, short of a wrap.
    fn row_before(&self) -> Option<usize> {
        let from = if self.playing_left { self.upcoming_from() } else { self.current_index? };
        from.checked_sub(1)
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
        self.shuffled();
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
        self.shuffled();
    }

    /// The current row leads a fresh shuffle, except a gap left at the end, after which the queue
    /// still ends.
    fn shuffled(&mut self) {
        if !self.playing_left || self.current_index.is_some() {
            self.current_index = Some(0);
        }
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

    pub fn to_persistable(&self) -> PersistableQueue {
        PersistableQueue {
            track_ids: self.play_ids().to_vec(),
            current_index: current_index_to_i32(self.current_index),
        }
    }
}

#[cfg(test)]
#[path = "../tests/queue_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/queue_entry_tests.rs"]
mod entry_tests;
