//! Internal data structures + constants used by the Playlists grid and
//! Playlist Detail submodules.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::entities::playlist::PlaylistStats;
use crate::entities::smart_criteria::SmartCriteria;
use crate::entities::track::TrackListRow as RsTrackListRow;

/// A playlist's pre-lowercased name, computed once per `fetch_grid` so the
/// per-keystroke filter walk and the name sort allocate nothing.
/// Positionally aligned with [`GridData::playlists`].
pub(super) struct PlaylistSearchKey {
    pub name_lc: String,
}

/// The grid's canonical data: the playlist list plus its pre-lowercased
/// search / sort keys, kept together behind one `Arc` so a rebuild is a
/// single refcount bump (and the two halves can never drift out of sync).
pub(super) struct GridData {
    pub playlists: Vec<PlaylistStats>,
    pub keys: Vec<PlaylistSearchKey>,
    /// Positionally aligned with [`GridData::playlists`]. `true` iff the row is
    /// a smart playlist whose criteria can be moved by a play-count flush
    /// (`SmartCriteria::depends_on_play_stats`); always `false` for regular
    /// playlists. Computed once here (one criteria parse per smart row) so the
    /// `stats_changed` gate is a cheap flag scan and the stats-only refresh can
    /// recount just the rows a play stat change can affect.
    pub stat_dependent: Vec<bool>,
}

impl GridData {
    pub(super) fn new(playlists: Vec<PlaylistStats>) -> Self {
        let keys = playlists
            .iter()
            .map(|p| PlaylistSearchKey {
                name_lc: p.name.to_lowercase(),
            })
            .collect();
        let stat_dependent = playlists
            .iter()
            .map(|p| {
                p.is_smart
                    && SmartCriteria::from_json_opt(p.smart_criteria.as_deref())
                        .depends_on_play_stats()
            })
            .collect();
        Self {
            playlists,
            keys,
            stat_dependent,
        }
    }

    /// Whether any cached row is a stat-dependent smart playlist — the gate for
    /// the `stats_changed` refresh.
    pub(super) fn has_stat_dependent(&self) -> bool {
        self.stat_dependent.iter().any(|&b| b)
    }

    /// `(track_count, total_duration_ms, stat_dependent)` for the row with `id`,
    /// if present. Backs the stats-only refresh: stat-dependent rows are
    /// recounted, others carry these cached counts forward. `stat_dependent` is
    /// `false` for regular playlists, so the same lookup gates the open-detail
    /// refresh too.
    pub(super) fn row_stats_by_id(&self, id: i64) -> Option<(i32, i64, bool)> {
        self.playlists.iter().position(|p| p.id == id).map(|i| {
            let p = &self.playlists[i];
            (p.track_count, p.total_duration_ms, self.stat_dependent[i])
        })
    }
}

/// Memoized filter + sort result — playlist indices in display order,
/// plus the `(filter, sort_field, sort_dir)` that produced them. A pure
/// `columns-changed` re-chunk reuses `indices` without re-filtering /
/// re-sorting; a filter or sort change recomputes. Cleared whenever
/// `fetch_grid` replaces the grid data.
pub(super) struct GridIndexCache {
    pub filter: String,
    pub sort_field: String,
    pub sort_dir: String,
    pub indices: Vec<usize>,
}

impl GridIndexCache {
    pub(super) fn matches(&self, filter: &str, sort_field: &str, sort_dir: &str) -> bool {
        self.filter == filter && self.sort_field == sort_field && self.sort_dir == sort_dir
    }
}

/// Grid-side state — the canonical playlist data the card grid derives
/// from.
pub(super) struct PlaylistGridState {
    pub data: Mutex<Arc<GridData>>,
    pub index_cache: Mutex<Option<GridIndexCache>>,
}

/// Detail-side state — the currently-open playlist's cached track list.
pub(super) struct PlaylistDetailState {
    /// Tracks in current display order (after the user's sort) — the
    /// **displayed** (filter-applied) subset, kept in lockstep with the
    /// Slint `tracks` model so the generic selection/sort logic stays
    /// valid. Mirrors `AlbumDetailState::tracks`.
    pub tracks: Mutex<Vec<RsTrackListRow>>,
    /// Canonical full track set for this playlist, in display-sort order.
    /// `tracks` holds only the displayed subset; `apply_filtered_detail`
    /// re-derives `tracks` by walking this through the current filter.
    /// Equal to `tracks` whenever no filter is active.
    pub all_tracks: Mutex<Vec<RsTrackListRow>>,
    /// Track ids in CANONICAL position (insertion / drag-reorder) order.
    /// Drag-reorder mutates this **optimistically** before the DB write
    /// lands, so the `"position"` sort can rebuild `tracks` from this
    /// without a re-fetch. Always in lock-step with the playlist's
    /// `playlist_items.position` column on disk modulo the in-flight
    /// optimistic write.
    pub position_order: Mutex<Vec<i64>>,
    /// Playlist id currently shown in the detail view (`-1` = none).
    pub playlist_id: Mutex<i64>,
    /// Selection set currently *stamped* onto the Slint row model — same
    /// diff-and-write-back contract as `AlbumDetailState::applied_selection`.
    pub applied_selection: Mutex<HashSet<i32>>,
    /// Live lowercased filter needle, mirroring `PlaylistDetail.filter`.
    /// Lets the re-fetch path (`refresh_detail`) re-apply the filter to
    /// fresh data without round-tripping the UI thread for the property
    /// read. Cleared on fresh-open. Mirrors `ArtistDetailState::filter`.
    pub filter: Mutex<String>,
}

/// Square decode size (px) for the Playlists **grid card** tiles. Matches
/// the Albums grid tier — playlists in the grid render at the same
/// per-card footprint as albums.
pub(super) const GRID_COVER_SIZE: u32 = 448;

/// Fallback LRU capacity for the **grid** cover cache, used at
/// construction and whenever the display can't be queried. Mirrors the
/// Albums tier.
pub(super) const DEFAULT_GRID_COVER_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("DEFAULT_GRID_COVER_CAP > 0"),
};

/// How many leading playlists' covers `fetch_grid` prewarms before the
/// grid first paints. Same rationale as the Albums tier — covers the first
/// screenful at any reasonable column count.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
