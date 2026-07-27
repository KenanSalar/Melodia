//! Per-view persistence of track-list column state.
//!
//! Each consumer of the reusable `TrackList` Slint component (Tracks,
//! Browse, Favorites, Search and the Album / Artist / Genre / Playlist
//! detail views) owns its own per-view Slint global mirroring the same
//! column-width and column-visibility shape. This module captures the
//! hydrate-on-startup / snapshot-on-shutdown flow generically, so adding a
//! new view is a matter of:
//!
//! 1. Declaring the new global in `melodia-ui/ui/globals.slint` mirroring `Tracks`.
//! 2. Adding a `view_id::*` constant and one
//!    [`impl_track_list_column_state!`] invocation below.
//! 3. Calling the generated `hydrate_*_view` at startup and
//!    `snapshot_*_view` on shutdown.
//!
//! Every view shares the identical hydrate/snapshot body — the only
//! per-view variation is the Slint global type, the settings view-id, and
//! (for detail views) one column the UI can no longer toggle. Rather than
//! hand-copy that body eight times, [`impl_track_list_column_state!`]
//! generates the `TrackListColumnState` impl plus the two shorthand
//! functions from those few parameters.
//!
//! Persistence rides on `views.json`:
//! `ViewStateData.view_column_widths: HashMap<String, ColumnWidths>` and
//! `ViewStateData.view_columns: HashMap<String, Vec<String>>`. Both are
//! already keyed by an arbitrary view-id, so adding a new view requires no
//! schema change.

use std::collections::HashSet;

use slint::ComponentHandle;

use crate::AppWindow;
use crate::{
    AlbumDetail, ArtistDetail, Browse, Favorites, GenreDetail, PlaylistDetail, RecentlyPlayed,
    Search, Tracks,
};
use crate::services::settings::ColumnWidths;
use crate::services::view_state::ViewStateData;

/// The Slint-side surface of a per-view track-list global, abstracted so the
/// hydrate/snapshot helpers can drive any number of views with a single code
/// path. Each generated global type (Tracks, Browse, …) gets its own `impl`
/// block — emitted by [`impl_track_list_column_state!`] — that just routes
/// through the auto-generated `get_*` / `set_*` accessors.
pub trait TrackListColumnState {
    fn get_widths(&self) -> ColumnWidths;
    fn set_widths(&self, w: &ColumnWidths);

    /// The user-toggleable column ids that are currently visible, in display
    /// order. Always-visible columns (e.g. title and length on the Tracks
    /// view) are excluded by design — they're not part of the toggle popup,
    /// and per-view lock policies differ (Album Detail keeps `#` always-on,
    /// Favorites locks nothing). Both write paths into
    /// `views.json`'s `view_columns[view_id]` — toggle-column callbacks and the
    /// shutdown snapshot — go through this method so the on-disk shape
    /// stays consistent within a view.
    fn snapshot_visible(&self) -> Vec<String>;

    /// Apply a set of visible column ids to the global's `show-*` flags.
    /// Ids not present in the set become `false`.
    fn apply_visible(&self, visible: &HashSet<&str>);
}

/// Apply persisted widths and visibility for a given view-id from
/// `views.json` to the supplied handle. Missing entries leave the Slint
/// defaults in place — matches first-launch behaviour.
pub fn hydrate(view_id: &str, vs: &ViewStateData, h: &dyn TrackListColumnState) {
    if let Some(w) = vs.view_column_widths.get(view_id) {
        h.set_widths(w);
    }
    if let Some(visible) = vs.view_columns.get(view_id) {
        let set: HashSet<&str> = visible.iter().map(std::string::String::as_str).collect();
        h.apply_visible(&set);
    }
}

/// Snapshot the current column widths off the handle, ready to be written
/// back into `views.json`'s `view_column_widths[view_id]` on shutdown. The
/// resize-handle drag clamps to each column's min/max in
/// `track-list-header.slint`, so the persisted values are always in range.
pub fn snapshot_widths(h: &dyn TrackListColumnState) -> ColumnWidths {
    h.get_widths()
}

/// Convenience: snapshot both widths and visibility into the matching
/// `views.json` entries for `view_id`. Mutates `vs` in place; the caller
/// is responsible for writing it back to disk.
pub fn snapshot_into_view_state(
    view_id: &str,
    vs: &mut ViewStateData,
    h: &dyn TrackListColumnState,
) {
    vs.view_column_widths
        .insert(view_id.to_owned(), h.get_widths());
    vs.view_columns
        .insert(view_id.to_owned(), h.snapshot_visible());
}

/// View-id constants. Centralised so the spelling stays consistent between
/// the hydrate path, the shutdown snapshot path, and the per-feature
/// callbacks (e.g. `Tracks.toggle-column` writing `view_columns["tracks"]`).
pub mod view_id {
    pub const TRACKS: &str = "tracks";
    pub const BROWSE: &str = "browse";
    pub const ALBUM_DETAIL: &str = "album_detail";
    pub const ARTIST_DETAIL: &str = "artist_detail";
    pub const GENRE_DETAIL: &str = "genre_detail";
    pub const PLAYLIST_DETAIL: &str = "playlist_detail";
    pub const FAVORITES: &str = "favorites";
    pub const RECENTLY_PLAYED: &str = "recently_played";
    pub const SEARCH: &str = "search";
    // Entity grids — `view_columns` doesn't apply (no track-list columns),
    // but `view_sort` does: the grid header's sort is persisted per grid.
    pub const ALBUMS: &str = "albums";
    pub const ARTISTS: &str = "artists";
    pub const GENRES: &str = "genres";
}

/// Force a detail-view's locked column off, regardless of what
/// `settings.json` says — guards against a hand-edited file re-enabling a
/// column the UI can no longer toggle (Album Detail's `album`, Artist
/// Detail's `artist`, Genre Detail's `genre`, all of which are redundant
/// since every row shares that value). Matched against the literal column
/// ident passed to [`impl_track_list_column_state!`].
macro_rules! force_locked_column_off {
    ($self:ident, album) => {
        $self.set_show_album(false);
    };
    ($self:ident, artist) => {
        $self.set_show_artist(false);
    };
    ($self:ident, genre) => {
        $self.set_show_genre(false);
    };
}

/// Generate the [`TrackListColumnState`] impl for a Slint global plus the
/// matching `hydrate_*_view` / `snapshot_*_view` shorthand functions.
///
/// `get_widths` / `set_widths` / `snapshot_visible` / `apply_visible` are
/// byte-identical across every view; the only variation is an optional
/// `locked = <column>` for the three detail views, where that column is
/// excluded from the persisted visible set and force-disabled on apply.
macro_rules! impl_track_list_column_state {
    (
        $global:ident, $view_id:ident, $hydrate:ident, $snapshot:ident
        $(, locked = $locked:ident)?
    ) => {
        impl TrackListColumnState for $global<'_> {
            fn get_widths(&self) -> ColumnWidths {
                ColumnWidths {
                    number: f64::from(self.get_w_number()),
                    title: f64::from(self.get_w_title()),
                    artist: f64::from(self.get_w_artist()),
                    album: f64::from(self.get_w_album()),
                    genre: f64::from(self.get_w_genre()),
                    year: f64::from(self.get_w_year()),
                    length: f64::from(self.get_w_length()),
                }
            }

            fn set_widths(&self, w: &ColumnWidths) {
                self.set_w_number(px_to_slint(w.number));
                self.set_w_title(px_to_slint(w.title));
                self.set_w_artist(px_to_slint(w.artist));
                self.set_w_album(px_to_slint(w.album));
                self.set_w_genre(px_to_slint(w.genre));
                self.set_w_year(px_to_slint(w.year));
                self.set_w_length(px_to_slint(w.length));
            }

            fn snapshot_visible(&self) -> Vec<String> {
                let mut v = Vec::with_capacity(6);
                if self.get_show_number() {
                    v.push("number".to_owned());
                }
                if self.get_show_artwork() {
                    v.push("artwork".to_owned());
                }
                if self.get_show_artist() {
                    v.push("artist".to_owned());
                }
                if self.get_show_album() {
                    v.push("album".to_owned());
                }
                if self.get_show_genre() {
                    v.push("genre".to_owned());
                }
                if self.get_show_year() {
                    v.push("year".to_owned());
                }
                // A locked detail-view column is redundant (every row shares
                // that value) and unreachable in the toggle popup — never
                // write it back to settings.
                $( v.retain(|c| c.as_str() != stringify!($locked)); )?
                v
            }

            fn apply_visible(&self, visible: &HashSet<&str>) {
                self.set_show_number(visible.contains("number"));
                self.set_show_artwork(visible.contains("artwork"));
                self.set_show_artist(visible.contains("artist"));
                self.set_show_album(visible.contains("album"));
                self.set_show_genre(visible.contains("genre"));
                self.set_show_year(visible.contains("year"));
                $( force_locked_column_off!(self, $locked); )?
            }
        }

        /// Hydrate this view's column widths + visibility from `views.json`.
        /// Generated by [`impl_track_list_column_state!`].
        pub fn $hydrate(app: &AppWindow, vs: &ViewStateData) {
            let g = app.global::<$global>();
            hydrate(view_id::$view_id, vs, &g);
        }

        /// Snapshot this view's column widths + visibility into the matching
        /// `views.json` entries. The caller writes `vs` back to disk.
        /// Generated by [`impl_track_list_column_state!`].
        pub fn $snapshot(app: &AppWindow, vs: &mut ViewStateData) {
            let g = app.global::<$global>();
            snapshot_into_view_state(view_id::$view_id, vs, &g);
        }
    };
}

impl_track_list_column_state!(Tracks, TRACKS, hydrate_tracks_view, snapshot_tracks_view);
impl_track_list_column_state!(Browse, BROWSE, hydrate_browse_view, snapshot_browse_view);
impl_track_list_column_state!(
    AlbumDetail,
    ALBUM_DETAIL,
    hydrate_album_detail_view,
    snapshot_album_detail_view,
    locked = album
);
impl_track_list_column_state!(
    ArtistDetail,
    ARTIST_DETAIL,
    hydrate_artist_detail_view,
    snapshot_artist_detail_view,
    locked = artist
);
impl_track_list_column_state!(
    GenreDetail,
    GENRE_DETAIL,
    hydrate_genre_detail_view,
    snapshot_genre_detail_view,
    locked = genre
);
impl_track_list_column_state!(
    PlaylistDetail,
    PLAYLIST_DETAIL,
    hydrate_playlist_detail_view,
    snapshot_playlist_detail_view
);
impl_track_list_column_state!(
    Favorites,
    FAVORITES,
    hydrate_favorites_view,
    snapshot_favorites_view
);
impl_track_list_column_state!(
    RecentlyPlayed,
    RECENTLY_PLAYED,
    hydrate_recently_played_view,
    snapshot_recently_played_view
);
impl_track_list_column_state!(Search, SEARCH, hydrate_search_view, snapshot_search_view);

/// Narrow a persisted f64 column width (settings.json) to the f32 Slint uses.
/// Column widths are tens-to-hundreds of pixels; f32 has ample precision.
#[allow(
    clippy::cast_possible_truncation,
    reason = "column widths are small pixel counts; f32 mantissa is sufficient"
)]
fn px_to_slint(v: f64) -> f32 {
    v as f32
}
