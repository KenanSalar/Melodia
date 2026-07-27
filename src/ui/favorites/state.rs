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
    /// Strip rows in Rust shape — kept so click handlers can resolve
    /// `(id) -> entity` without re-fetching. Three independent caches,
    /// one per strip, each refreshed in lockstep on `refresh_strips`.
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

/// Most-Played strip tile size (px). The strip renders 160 px square cards,
/// and `FemtoVG` minifies with plain bilinear (no mipmaps), so staying near
/// the on-screen size keeps `image-fit: cover` clean without the album
/// grid's 448 px tier.
pub(super) const MOST_PLAYED_THUMB_SIZE: u32 = 180;

/// LRU capacity for the Most Played strip — `library::favorites::
/// get_most_played_favorites` is clamped to 10 results, so a cap of 16
/// covers post-refresh cycling without thrashing.
pub(super) const MOST_PLAYED_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(n) => n,
    None => panic!("MOST_PLAYED_THUMB_CAP > 0"),
};

/// Favorite Artists strip tile size (px). Larger than the Most-Played
/// tier because the circular avatar reads softer when downscaled less
/// aggressively. 200 px is the Tauri parity size for circular artist
/// chrome.
pub(super) const ARTIST_THUMB_SIZE: u32 = 200;

/// LRU capacity for the artist circular tiles — the strip's natural
/// length grows with library size (one per artist with ≥1 favourite),
/// so 32 covers a typical user's working set without unbounded growth.
pub(super) const ARTIST_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(32) {
    Some(n) => n,
    None => panic!("ARTIST_THUMB_CAP > 0"),
};

