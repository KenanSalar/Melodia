//! The set every source plays, written by one [`CorrectionWriter`] and read lock-free on the audio
//! thread under a sequence count. A set spans many cells, so a reader landing mid-write would
//! otherwise build a curve that is half one headphone's and half another's.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering, fence};

use crate::player::playback::dsp::AtomicF64;
use melodia_core::entities::correction::{
    CorrectionFilter, CorrectionFilters, FilterKind, MAX_CORRECTION_FILTERS,
};

/// An even sequence is a settled set and an odd one a write under way.
pub struct CorrectionShared {
    sequence: AtomicU64,
    len: AtomicUsize,
    cells: [FilterCell; MAX_CORRECTION_FILTERS],
}

impl CorrectionShared {
    /// A set newer than `last`, or `None` while a write is under way, which the caller retries at
    /// its next frame rather than spinning on the audio thread.
    pub(crate) fn read_if_changed(&self, last: Option<u64>) -> Option<(u64, PublishedSet)> {
        let before = self.sequence.load(Ordering::Acquire);
        if Some(before) == last || !before.is_multiple_of(2) {
            return None;
        }
        let len = self.len.load(Ordering::Relaxed);
        let filters = self.cells.each_ref().map(FilterCell::load);
        // Holds the cell loads above the recheck, so a write that overlapped them moves the count.
        fence(Ordering::Acquire);
        let after = self.sequence.load(Ordering::Relaxed);
        (after == before).then_some((before, PublishedSet { filters, len }))
    }
}

/// The only handle that changes the set. It isn't `Clone` and writes through `&mut self`, since
/// two writers interleaving could each leave the count even mid-write.
pub struct CorrectionWriter {
    shared: Arc<CorrectionShared>,
}

/// Starts empty, every source playing uncorrected until the first publish.
impl Default for CorrectionWriter {
    fn default() -> Self {
        let shared = CorrectionShared {
            sequence: AtomicU64::new(0),
            len: AtomicUsize::new(0),
            cells: std::array::from_fn(|_| FilterCell::new()),
        };
        Self { shared: Arc::new(shared) }
    }
}

impl CorrectionWriter {
    pub fn shared(&self) -> Arc<CorrectionShared> {
        Arc::clone(&self.shared)
    }

    pub fn publish(&mut self, filters: &CorrectionFilters) {
        self.write(filters.filters());
    }

    /// Every source fades to uncorrected.
    pub fn clear(&mut self) {
        self.write(&[]);
    }

    fn write(&mut self, filters: &[CorrectionFilter]) {
        let shared = &*self.shared;
        let settled = shared.sequence.load(Ordering::Relaxed);
        shared.sequence.store(settled + 1, Ordering::Relaxed);
        // A reader that sees any cell written below sees the count gone odd as well.
        fence(Ordering::Release);
        for (cell, filter) in shared.cells.iter().zip(filters) {
            cell.store(filter);
        }
        shared.len.store(filters.len(), Ordering::Relaxed);
        shared.sequence.store(settled + 2, Ordering::Release);
    }
}

/// A settled copy of the set, on the reader's stack.
pub(crate) struct PublishedSet {
    filters: [CorrectionFilter; MAX_CORRECTION_FILTERS],
    len: usize,
}

impl PublishedSet {
    pub(crate) fn filters(&self) -> &[CorrectionFilter] {
        &self.filters[..self.len]
    }
}

struct FilterCell {
    kind: AtomicU8,
    freq_hz: AtomicF64,
    gain_db: AtomicF64,
    q: AtomicF64,
}

impl FilterCell {
    fn new() -> Self {
        Self {
            kind: AtomicU8::new(0),
            freq_hz: AtomicF64::new(0.0),
            gain_db: AtomicF64::new(0.0),
            q: AtomicF64::new(0.0),
        }
    }

    fn store(&self, filter: &CorrectionFilter) {
        self.kind.store(kind_to_cell(filter.kind), Ordering::Relaxed);
        self.freq_hz.store(filter.freq_hz);
        self.gain_db.store(filter.gain_db);
        self.q.store(filter.q);
    }

    fn load(&self) -> CorrectionFilter {
        CorrectionFilter {
            kind: kind_from_cell(self.kind.load(Ordering::Relaxed)),
            freq_hz: self.freq_hz.load(),
            gain_db: self.gain_db.load(),
            q: self.q.load(),
        }
    }
}

fn kind_to_cell(kind: FilterKind) -> u8 {
    match kind {
        FilterKind::Peaking => 0,
        FilterKind::LowShelf => 1,
        FilterKind::HighShelf => 2,
    }
}

/// Only [`kind_to_cell`]'s codes are ever stored, so the fallback is a cell never written.
fn kind_from_cell(code: u8) -> FilterKind {
    match code {
        1 => FilterKind::LowShelf,
        2 => FilterKind::HighShelf,
        _ => FilterKind::Peaking,
    }
}
