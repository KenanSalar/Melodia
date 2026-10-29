/// The most records one undo may hold, a run merged into one notice counting as one. A removal
/// past it writes as it always has and says it can't be undone, so no hold grows with the library.
pub const UNDO_HOLD_CAP: usize = 10_000;

/// What a removal hands back for its undo.
#[derive(Debug)]
pub enum UndoCapture<T> {
    /// It removed nothing, so there is nothing to tell.
    Nothing,
    Held(T),
    /// It removed `rows`, too many to keep.
    OverCap {
        rows: usize,
    },
}
