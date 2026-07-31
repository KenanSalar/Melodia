//! Internal data structures used by the Favorites view. Mirrors
//! `src/ui/albums/state.rs` in shape — concentrated here so the public
//! [`crate::ui::favorites::FavoritesUi`] surface stays small.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use parking_lot::Mutex;

use crate::entities::artist::FavoriteArtist;
use crate::entities::track::{FavoriteStats, MostPlayedFavorite, TrackListRow as RsTrackListRow};
use crate::services::settings::{SortDir, ViewSort};

/// Per-section cached snapshots — every fetch lands here so callbacks
/// can recover the underlying Rust data without round-tripping through
/// Slint. Each field is mutated only on its respective fetch path; the
/// `Mutex<>` wrappers keep them callable from any tokio worker without
/// pinning the UI thread.
pub(crate) struct FavoritesUiState {
    /// All favourites in current sort order, *before* the in-memory
    /// search filter. The Slint side renders `tracks_filtered`; this
    /// retains the unfiltered list so a keystroke can rewalk without
    /// hitting `SQLite`.
    pub tracks_all: Mutex<Vec<RsTrackListRow>>,
    /// Active filter string — cached so the live-refresh subscriber
    /// (after a `library_changed_tx` tick) re-applies it without
    /// re-reading the Slint property from a tokio thread.
    pub filter: Mutex<String>,
    /// Active sort — written on `set-sort-*` callbacks, read on every
    /// re-fetch. Default mirrors the Slint global's defaults.
    pub sort: Mutex<ViewSort>,
    /// Hero stats — most recent successful `get_favorite_stats` result.
    /// Held so an empty Slint `mosaic-paths` write on section leave can
    /// be reverted on section re-enter without a DB round trip in the
    /// common case.
    pub stats: Mutex<FavoriteStats>,
    /// Grid-tab rows in Rust shape — kept so click handlers can resolve
    /// `(id) -> entity` without re-fetching, and so a filter keystroke or
    /// a column-count change re-chunks in memory. Two independent caches,
    /// one per grid, refreshed in lockstep on `refresh_grids`.
    pub most_played: Mutex<Vec<MostPlayedFavorite>>,
    pub fav_artists: Mutex<Vec<FavoriteArtist>>,
    /// Set of `TrackListRow.id`s currently `selected: true` on the
    /// Slint model. Same diff-then-write pattern Albums uses to keep
    /// selection updates O(changed) rather than O(rows).
    pub applied_selection: Mutex<HashSet<i32>>,
    /// Mosaic cover paths last composed into the hero blur. Guards against
    /// recomposing (up to 4 decodes + one blur) when a refresh yields the same
    /// covers — the common case for a played-track / in-view-toggle refresh.
    /// Reset on section-leave so a genuine re-enter recomposes.
    pub last_mosaic_paths: Mutex<Vec<String>>,
}

impl FavoritesUiState {
    pub(super) fn new() -> Self {
        Self {
            tracks_all: Mutex::new(Vec::new()),
            filter: Mutex::new(String::new()),
            sort: Mutex::new(ViewSort {
                field: "title".to_owned(),
                dir: SortDir::Asc,
            }),
            stats: Mutex::new(FavoriteStats {
                count: 0,
                total_duration_ms: 0,
                artwork_paths: Vec::new(),
            }),
            most_played: Mutex::new(Vec::new()),
            fav_artists: Mutex::new(Vec::new()),
            applied_selection: Mutex::new(HashSet::new()),
            last_mosaic_paths: Mutex::new(Vec::new()),
        }
    }
}

/// Mosaic-tile cache size (px). Each hero tile is rendered at ~70 px
/// (half of the 140 px hero tile, minus the gutter), so 128 px gives a
/// crisp downscale without paying the 384 px detail-tier cost.
pub(super) const MOSAIC_THUMB_SIZE: u32 = 128;

/// LRU capacity for the mosaic-tile cache. Small — at most four covers
/// live in the mosaic at once, but recently-displaced tiles can re-enter
/// when the top-4 most-played shifts. Each buffer is ~48 KiB.
pub(super) const MOSAIC_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(n) => n,
    None => panic!("MOSAIC_THUMB_CAP > 0"),
};

/// Grid-card tile size (px), shared by both grid tabs. These used to be
/// 180 / 200 px tiers sized for 160 px strip cards; the tabs draw the same
/// flex-filled cards the Albums and Artists grids do, which run well past
/// 260 px, and `FemtoVG` minifies with plain bilinear (no mipmaps) — so an
/// undersized tier is a visibly soft tile rather than a saving. Matches
/// `ui::albums::state::GRID_COVER_SIZE`.
pub(super) const GRID_THUMB_SIZE: u32 = 448;

/// LRU capacity per grid tier. Sized like the album grid's default: enough
/// for a screenful or two of cards, so scrolling re-decodes rather than the
/// cache growing with the library.
///
/// The two tiers are never both warm — the tabs are mutually exclusive and
/// `tab-changed` releases the one being left — so this bounds one tier's
/// worth of resident buffers at a time, not two.
///
/// A construction default only: `ui::favorites::tune_cache_for_display`
/// resizes both tiers against the real display once the window is live, the
/// same as the entity grids.
pub(super) const GRID_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("GRID_THUMB_CAP > 0"),
};

/// Most-Played grid tile size (px).
pub(super) const MOST_PLAYED_THUMB_SIZE: u32 = GRID_THUMB_SIZE;

/// Favorite Artists grid tile size (px). Same tier as Most Played — the
/// circular mask is applied at draw time, so the source needs no extra
/// resolution.
pub(super) const ARTIST_THUMB_SIZE: u32 = GRID_THUMB_SIZE;

/// How many covers to decode up front when a grid tab becomes visible.
///
/// A screenful or two, matching `ui::albums::state::GRID_PREWARM_AHEAD` — not
/// the tier's capacity. The grids are uncapped now, so prewarming everything
/// would decode a large library's whole favourite set at 448 px on every
/// `library_changed` tick, and all but the last `GRID_THUMB_CAP_N` of them
/// would be evicted by the prewarm itself before a single card asked for one.
/// The rest decode lazily as rows scroll in, which is what the `ListView`
/// virtualization is for.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;

