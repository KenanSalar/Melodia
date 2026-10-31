/// The most records one undo may hold, a run merged into one notice counting as one. A removal
/// past it writes as it always has and says it can't be undone, so no hold grows with the library.
pub const UNDO_HOLD_CAP: usize = 10_000;

/// Which removals a Settings switch turns undo off for, one per surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoKind {
    Playlists,
    Queue,
    Favorites,
    Radio,
    Equalizer,
    RecentSearches,
}

impl UndoKind {
    /// The spelling `settings.json` keeps for a kind turned off, so a renamed variant never
    /// repoints a saved choice.
    pub fn key(self) -> &'static str {
        match self {
            Self::Playlists => "playlists",
            Self::Queue => "queue",
            Self::Favorites => "favorites",
            Self::Radio => "radio",
            Self::Equalizer => "equalizer",
            Self::RecentSearches => "recent_searches",
        }
    }
}

/// How much a removal took, for the switch that quiets undo for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalSize {
    /// As many as its notice would count.
    Items(usize),
    /// A whole state, which no count describes: Clear all, Reset, a cleared or replaced queue.
    Whole,
}

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
