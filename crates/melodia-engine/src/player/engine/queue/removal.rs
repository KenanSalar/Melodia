//! Every way rows leave the queue, and the restores that put back what a removal held.

use std::collections::HashSet;

use melodia_core::utils::reinsert::reinsert;

use super::{QueueEntry, QueueState};

/// Result of [`QueueState::prune_missing`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneOutcome {
    /// Rows dropped from the play order.
    pub removed: usize,
    /// The playing row went, so the caller skips on.
    pub playing_was_removed: bool,
}

/// What [`QueueState::cut`] took, for [`QueueState::restore_cuts`] to put back. Ids and indices
/// only, so it costs what its rows did in the queue.
#[derive(Debug)]
pub struct QueueCut {
    /// In play order, each where it sat in both orders the moment it was cut.
    rows: Vec<CutRow>,
    /// `slot` is the index in `rows` of the row that was current, where the cut took it.
    cursor: HeldCursor,
    shuffled: bool,
}

impl QueueCut {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[derive(Debug)]
struct CutRow {
    track_id: i64,
    play_index: usize,
    original_index: usize,
}

/// What [`QueueState::take_cleared`] moved out, for [`QueueState::restore_cleared`] or
/// [`QueueState::restore_replaced`].
#[derive(Debug)]
pub struct ClearedQueue {
    tracks: Vec<i64>,
    play_order: Vec<usize>,
    original_order: Vec<usize>,
    /// `slot` is the current row's slot in `tracks`.
    cursor: HeldCursor,
    shuffled: bool,
    hand_edited: bool,
}

/// The cursor as a removal found it. Its restore puts it back only while nothing has been seated
/// since: once a row has started, that row stays current.
#[derive(Debug, Clone, Copy)]
struct HeldCursor {
    slot: Option<usize>,
    playing_left: bool,
    seats: u64,
}

impl QueueState {
    /// Takes the rows `entries` name, wherever they sit now, handing back what puts them back, or
    /// `None` where none is still queued. Taking the playing row leaves a gap where it was
    /// (`playing_left`), the row that took its place playing next.
    pub fn cut(&mut self, entries: &[QueueEntry]) -> Option<QueueCut> {
        let gone =
            self.slot_mask(entries.iter().filter_map(|entry| entry.0.checked_sub(self.retired)));
        let rows = self.rows_at(&gone);
        if rows.is_empty() {
            return None;
        }
        let current_row = rows.iter().position(|row| Some(row.play_index) == self.current_index);
        let cursor = self.held_cursor(current_row);

        self.playing_left |= current_row.is_some();
        self.hand_edited = true;
        self.drop_rows(&gone);
        Some(QueueCut { rows, cursor, shuffled: self.shuffle_enabled })
    }

    /// The cursor as it stands, `slot` naming the current row in the hold's own terms.
    fn held_cursor(&self, slot: Option<usize>) -> HeldCursor {
        HeldCursor { slot, playing_left: self.playing_left, seats: self.seats }
    }

    /// Marks `slots` by index, slots being dense, so walking a library-sized queue hashes no row.
    fn slot_mask(&self, slots: impl Iterator<Item = usize>) -> Vec<bool> {
        let mut mask = vec![false; self.tracks.len()];
        for slot in slots {
            if let Some(marked) = mask.get_mut(slot) {
                *marked = true;
            }
        }
        mask
    }

    /// The queued rows marked in `gone`, in play order.
    fn rows_at(&self, gone: &[bool]) -> Vec<CutRow> {
        let mut original_index: Vec<(usize, usize)> = self
            .original_order
            .iter()
            .enumerate()
            .filter(|&(_, &slot)| gone[slot])
            .map(|(index, &slot)| (slot, index))
            .collect();
        original_index.sort_unstable();
        self.play_order
            .iter()
            .enumerate()
            .filter(|&(_, &slot)| gone[slot])
            .filter_map(|(play_index, &slot)| {
                let found = original_index.binary_search_by_key(&slot, |&(slot, _)| slot).ok()?;
                Some(CutRow {
                    original_index: original_index[found].1,
                    track_id: self.tracks[slot],
                    play_index,
                })
            })
            .collect()
    }

    /// Drops the rows marked in `gone` from both orders and lands the cursor on the row that took
    /// the current one's place. `tracks` keeps the slots so every other index stays valid; `clear`
    /// drops them.
    fn drop_rows(&mut self, gone: &[bool]) {
        let survivors_before_current = self
            .current_index
            .map(|ci| self.play_order.iter().take(ci).filter(|&&slot| !gone[slot]).count());
        self.play_order.retain(|&slot| !gone[slot]);
        self.original_order.retain(|&slot| !gone[slot]);
        self.land_cursor(survivors_before_current);
        self.order_changed();
    }

    /// Puts back what a run of [`Self::cut`]s took, newest first, so each lands in the coordinates
    /// of the moment it was made.
    pub fn restore_cuts(&mut self, cuts: Vec<QueueCut>) {
        for cut in cuts.into_iter().rev() {
            self.restore_cut(cut);
        }
    }

    /// The rows go back under fresh slots, so no entry is handed out twice. Under shuffle off a
    /// cut made shuffled takes its insertion order, which the unshuffle rebuilt the play order from.
    fn restore_cut(&mut self, cut: QueueCut) {
        let QueueCut { rows, cursor, shuffled } = cut;
        let live = self.current_slot();
        let start = self.tracks.len();
        self.tracks.extend(rows.iter().map(|row| row.track_id));

        let mut by_original: Vec<(usize, usize)> =
            rows.iter().zip(start..).map(|(row, slot)| (row.original_index, slot)).collect();
        by_original.sort_unstable();
        let by_play = if shuffled && !self.shuffle_enabled {
            by_original.clone()
        } else {
            rows.iter().zip(start..).map(|(row, slot)| (row.play_index, slot)).collect()
        };
        self.original_order = reinsert(std::mem::take(&mut self.original_order), by_original);
        self.play_order = reinsert(std::mem::take(&mut self.play_order), by_play);

        match cursor.slot {
            Some(row) if self.seats == cursor.seats => {
                self.place_cursor(Some(start + row), cursor.playing_left);
            }
            _ => self.place_cursor(live, self.playing_left),
        }
        self.order_changed();
    }

    /// Makes the row in `slot` current without counting a seat, a restore putting the cursor back
    /// rather than starting anything.
    fn place_cursor(&mut self, slot: Option<usize>, playing_left: bool) {
        self.current_index = slot.and_then(|slot| self.play_index_of_slot(slot));
        self.playing_left = playing_left;
    }

    /// Puts the cursor on the row that took the current one's place, `survivors_before` rows in.
    /// Past the end that is no row while the gap holds there, else the last.
    fn land_cursor(&mut self, survivors_before: Option<usize>) {
        let len = self.play_order.len();
        self.current_index = survivors_before.and_then(|before| {
            if before < len {
                Some(before)
            } else if self.playing_left {
                None
            } else {
                len.checked_sub(1)
            }
        });
    }

    pub fn clear(&mut self) {
        drop(self.take_cleared());
    }

    /// Empties the queue, handing back what it held, or `None` where nothing was queued. Moved
    /// rather than copied, so the hold costs what the queue did a moment before.
    pub fn take_cleared(&mut self) -> Option<ClearedQueue> {
        let held = ClearedQueue {
            cursor: self.held_cursor(self.current_slot()),
            shuffled: self.shuffle_enabled,
            hand_edited: std::mem::take(&mut self.hand_edited),
            tracks: std::mem::take(&mut self.tracks),
            play_order: std::mem::take(&mut self.play_order),
            original_order: std::mem::take(&mut self.original_order),
        };
        self.retired += held.tracks.len();
        self.current_index = None;
        self.playing_left = false;
        self.order_changed();
        (!held.play_order.is_empty()).then_some(held)
    }

    /// Makes `ids` the queue with the row at `start` playing, handing back the queue it took where
    /// that one was built by hand and differs from `ids`. A play over a queue only a play built
    /// holds nothing: a notice on every double-click teaches the eye to skip the one that matters.
    pub fn replace(&mut self, ids: &[i64], start: usize) -> Option<ClearedQueue> {
        let worth_holding = self.hand_edited && !self.already_holds(ids);
        let taken = self.take_cleared();
        self.insert_at(0, ids);
        self.hand_edited = false;
        self.seat(start);
        taken.filter(|_| worth_holding)
    }

    /// Shuffled, only the insertion order can match, the replace randomizing the play order;
    /// unshuffled, the play order is the one a move edits.
    fn already_holds(&self, ids: &[i64]) -> bool {
        let order = if self.shuffle_enabled { &self.original_order } else { &self.play_order };
        order.iter().map(|&slot| self.tracks.get(slot)).eq(ids.iter().map(Some))
    }

    /// Puts back what a run of [`Self::take_cleared`]s took, newest first, each ahead of anything
    /// queued since, so the oldest ends up first.
    pub fn restore_cleared(&mut self, cleared: Vec<ClearedQueue>) {
        for held in cleared.into_iter().rev() {
            self.restore_one_cleared(held);
        }
    }

    fn restore_one_cleared(&mut self, held: ClearedQueue) {
        let live = self.current_slot();
        let cursor = held.cursor;
        let (start, rows) = self.splice_ahead(held);
        if self.seats == cursor.seats {
            self.place_held_cursor(cursor, start, rows);
        } else {
            self.place_cursor(live, self.playing_left);
        }
        self.order_changed();
    }

    /// Puts back the queue a [`Self::replace`] took in place of the one it built, its cursor where it
    /// was whatever has been seated since: the play seated a row, and its undo takes that back too.
    /// A hold made unshuffled is shuffled where shuffle has come on since, or the lit button would
    /// walk it straight through.
    pub fn restore_replaced<R: rand::Rng + ?Sized>(&mut self, held: ClearedQueue, rng: &mut R) {
        self.clear();
        let (cursor, shuffled) = (held.cursor, held.shuffled);
        let (start, rows) = self.splice_ahead(held);
        self.place_held_cursor(cursor, start, rows);
        self.order_changed();
        if self.shuffle_enabled && !shuffled {
            self.shuffle_play_order_in_place(rng, /* anchor_to_current */ true);
        }
    }

    /// Under fresh slots, as [`Self::restore_cut`] does, and in insertion order where shuffle has
    /// gone off since. Answers its first slot and its row count.
    fn splice_ahead(&mut self, held: ClearedQueue) -> (usize, usize) {
        let ClearedQueue { tracks, play_order, original_order, shuffled, hand_edited, .. } = held;
        let start = self.tracks.len();
        self.tracks.extend(tracks);

        let held_play =
            if shuffled && !self.shuffle_enabled { &original_order } else { &play_order };
        self.play_order.splice(0..0, held_play.iter().map(|slot| start + slot));
        self.original_order.splice(0..0, original_order.iter().map(|slot| start + slot));
        self.hand_edited |= hand_edited;
        (start, play_order.len())
    }

    fn place_held_cursor(&mut self, cursor: HeldCursor, start: usize, rows: usize) {
        // A gap held at the end leaves what was queued since still to play.
        let first_since = || self.play_order.get(rows).copied();
        let slot = cursor.slot.map(|slot| start + slot);
        let slot = if cursor.playing_left { slot.or_else(first_since) } else { slot };
        self.place_cursor(slot, cursor.playing_left);
    }

    /// Drops the rows whose track is in `ids_to_remove`, their slots staying in `tracks` so every
    /// survivor keeps its [`QueueEntry`]. The current index lands on the next survivor, else the
    /// last, or past a gap's end on none: a prune keeps a gap but never opens one, the caller
    /// moving on instead. Bumps `version` only when a row went.
    pub fn prune_missing(&mut self, ids_to_remove: &HashSet<i64>) -> PruneOutcome {
        let missing =
            |&slot: &usize| self.tracks.get(slot).is_some_and(|id| ids_to_remove.contains(id));
        let gone = self.slot_mask(self.play_order.iter().copied().filter(missing));
        let removed = gone.iter().filter(|&&marked| marked).count();
        if removed == 0 {
            return PruneOutcome::default();
        }

        let playing_was_removed =
            !self.playing_left && self.current_slot().is_some_and(|slot| gone[slot]);
        self.drop_rows(&gone);
        PruneOutcome { removed, playing_was_removed }
    }
}
