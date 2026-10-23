//! Per-view persistence of track-list column state.
//!
//! Every consumer of the reusable `TrackList` component owns a per-view Slint global holding one
//! `TrackColumns` value, and the hydrate-on-startup / snapshot-on-shutdown body is identical across
//! all of them. What varies is the global type, the view-id, the columns a first launch hides and,
//! for a detail view, the one column the UI can no longer toggle. `track_list_views!` takes one row
//! of those per view and generates each `TrackListColumnState` impl plus [`hydrate_all`] and
//! [`snapshot_all`], so a new view is a global, a `view_id::*` const and one row.
//!
//! Persistence rides on `views.json`'s `view_column_widths`, `view_columns` and `view_marks`, all
//! keyed by an arbitrary view-id, so a new view needs no schema change. **The defaults are
//! Rust's**, each view's visible set here and the widths in `ColumnWidths::default()`: the globals
//! declare none, so a view left out of [`hydrate_all`] would have no columns at all. One table
//! feeding both passes is what keeps that from happening.

use std::collections::{BTreeMap, HashSet};

use slint::{ComponentHandle, ModelRc, SharedString};

use crate::ui::callbacks::macros::spawn_blocking_logged;
use melodia_app::library;
use melodia_app::services::settings::ColumnWidths;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::{
    AlbumDetail, ArtistDetail, Browse, Favorites, GenreDetail, PlaylistDetail, RecentlyPlayed,
    Search, Tracks,
};
use melodia_ui::{AppWindow, TrackColumns};

/// Every column the popup can toggle, by the id `view_columns` stores. Title and Length are always
/// shown and have none. [`columns_from`] and [`TrackListColumnState::snapshot_visible`] both walk
/// the `show_*` fields in this order.
const TOGGLEABLE: [&str; 6] = ["number", "artwork", "artist", "album", "genre", "year"];

/// The row marks the popup toggles beside the columns, by the id `view_marks` stores: the heart
/// beside a favourite's title, the one its duration gives way to on hover, and the hover star
/// strip. Their own map rather than more `view_columns` ids, which `ViewStateData::view_marks`
/// argues. [`columns_from`] and [`TrackListColumnState::snapshot_marks`] walk them in this order.
const MARKS: [&str; 3] = ["favorite_mark", "favorite_button", "rating_strip"];

/// The Slint-side surface of a per-view track-list global, so the hydrate, snapshot and toggle
/// paths drive any number of views down one path. Each generated global gets its own `impl` from
/// `track_list_views!`, routing through the accessors.
pub trait TrackListColumnState {
    /// The key every `views.json` map holds this view under.
    const VIEW_ID: &'static str;
    /// The toggleable ids, columns or marks, a view hides until it has a set of its own saved.
    const HIDDEN_BY_DEFAULT: &'static [&'static str];
    /// The column a detail view hides for good, every row sharing its value.
    const LOCKED: Option<&'static str>;

    fn read_columns(&self) -> TrackColumns;
    fn write_columns(&self, columns: TrackColumns);

    /// The user-toggleable column ids currently visible, in display order.
    /// Always-visible columns are excluded by design — they aren't in the toggle popup,
    /// and the lock policy differs per view. Both writers into `view_columns[view_id]`
    /// go through this, so the on-disk shape stays consistent within a view.
    fn snapshot_visible(&self) -> Vec<String> {
        let columns = self.read_columns();
        let shown = [
            columns.show_number,
            columns.show_artwork,
            columns.show_artist,
            columns.show_album,
            columns.show_genre,
            columns.show_year,
        ];
        shown_ids(TOGGLEABLE, shown)
            .filter(|&id| Self::LOCKED != Some(id))
            .map(str::to_owned)
            .collect()
    }

    /// The row marks currently shown, in [`MARKS`] order, for `view_marks[view_id]`.
    fn snapshot_marks(&self) -> Vec<String> {
        let columns = self.read_columns();
        let shown =
            [columns.show_favorite_mark, columns.show_favorite_button, columns.show_rating_strip];
        shown_ids(MARKS, shown).map(str::to_owned).collect()
    }
}

/// The ids whose flag is set, in order.
fn shown_ids<const N: usize>(
    ids: [&'static str; N],
    shown: [bool; N],
) -> impl Iterator<Item = &'static str> {
    ids.into_iter().zip(shown).filter_map(|(id, shown)| shown.then_some(id))
}

/// The callbacks every track list's global declares alike, for the wiring
/// [`crate::ui::callbacks::track_list`] writes once for all of them. Method names stay apart from
/// the generated accessors, as [`crate::ui::list_selection::RowSelectionView`]'s do.
pub trait TrackListActions {
    fn bind_play_next(&self, f: impl FnMut(ModelRc<i32>) + 'static);
    fn bind_add_to_queue(&self, f: impl FnMut(ModelRc<i32>) + 'static);
    fn bind_toggle_column(&self, f: impl FnMut(SharedString) + 'static);
}

/// Persist the set the toggle popup's `toggled` id belongs to, the marks or the columns. The popup
/// has already repainted, so what is left is the write.
pub fn persist_toggled<T: TrackListColumnState>(state: &AppState, h: &T, toggled: &str) {
    let view_id = T::VIEW_ID.to_owned();
    let s = state.clone();
    if MARKS.contains(&toggled) {
        let marks = h.snapshot_marks();
        spawn_blocking_logged!(
            s,
            "track_list_view::persist_marks",
            library::settings::update_view_marks(&s.paths, view_id, marks)
        );
    } else {
        let columns = h.snapshot_visible();
        spawn_blocking_logged!(
            s,
            "track_list_view::persist_columns",
            library::settings::update_view_columns(&s.paths, view_id, columns)
        );
    }
}

/// Write the view's columns to the handle: `views.json`'s widths and visibility where it has
/// them, the defaults where it doesn't. The marks and the columns fall back apart, an install
/// having saved columns long before it had marks to save.
fn hydrate<T: TrackListColumnState>(vs: &ViewStateData, h: &T) {
    let widths = vs.view_column_widths.get(T::VIEW_ID).cloned().unwrap_or_default();
    let saved_columns = saved_ids(&vs.view_columns, T::VIEW_ID);
    let saved_marks = saved_ids(&vs.view_marks, T::VIEW_ID);
    let shown = |id: &str| {
        let saved = if MARKS.contains(&id) { &saved_marks } else { &saved_columns };
        match saved {
            Some(visible) => visible.contains(id),
            None => !T::HIDDEN_BY_DEFAULT.contains(&id),
        }
    };
    h.write_columns(columns_from(&widths, shown, T::LOCKED));
}

fn saved_ids<'a>(
    saved: &'a BTreeMap<String, Vec<String>>,
    view_id: &str,
) -> Option<HashSet<&'a str>> {
    saved.get(view_id).map(|ids| ids.iter().map(String::as_str).collect())
}

/// Snapshot widths, visible columns and marks into the view's `views.json` entries, mutating `vs`
/// in place. The caller writes it back to disk.
fn snapshot<T: TrackListColumnState>(vs: &mut ViewStateData, h: &T) {
    vs.view_column_widths.insert(T::VIEW_ID.to_owned(), widths_from(&h.read_columns()));
    vs.view_columns.insert(T::VIEW_ID.to_owned(), h.snapshot_visible());
    vs.view_marks.insert(T::VIEW_ID.to_owned(), h.snapshot_marks());
}

/// The columns for `widths`, each toggleable column and mark shown as `shown` answers for its id.
/// A locked column is off whatever `shown` says, against a hand-edit naming one the UI no longer
/// offers.
fn columns_from(
    widths: &ColumnWidths,
    shown: impl Fn(&str) -> bool,
    locked: Option<&str>,
) -> TrackColumns {
    let [show_number, show_artwork, show_artist, show_album, show_genre, show_year] =
        TOGGLEABLE.map(|id| shown(id) && locked != Some(id));
    let [show_favorite_mark, show_favorite_button, show_rating_strip] = MARKS.map(shown);
    TrackColumns {
        w_number: px_to_slint(widths.number),
        w_title: px_to_slint(widths.title),
        w_artist: px_to_slint(widths.artist),
        w_album: px_to_slint(widths.album),
        w_genre: px_to_slint(widths.genre),
        w_year: px_to_slint(widths.year),
        w_length: px_to_slint(widths.length),
        show_number,
        show_artwork,
        show_artist,
        show_album,
        show_genre,
        show_year,
        show_favorite_mark,
        show_favorite_button,
        show_rating_strip,
    }
}

fn widths_from(columns: &TrackColumns) -> ColumnWidths {
    ColumnWidths {
        number: f64::from(columns.w_number),
        title: f64::from(columns.w_title),
        artist: f64::from(columns.w_artist),
        album: f64::from(columns.w_album),
        genre: f64::from(columns.w_genre),
        year: f64::from(columns.w_year),
        length: f64::from(columns.w_length),
    }
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
    /// The Favorites page's Favorite Artists *tab*, whose sort is its own —
    /// [`FAVORITES`] is the Songs tab's, over track columns this grid has no notion of.
    pub const FAVORITE_ARTISTS: &str = "favorite_artists";
    /// Radio's Favorites tab. Its sibling tab has no entry: Recently Played's order *is*
    /// the page, so there is no sort state to persist.
    pub const RADIO_FAVORITES: &str = "radio_favorites";
    /// The station page. `last_detail_ids` only — a station has no track columns and no
    /// sort, and only a station with a database row can be named here at all.
    pub const RADIO_DETAIL: &str = "radio_detail";
}

macro_rules! locked_column {
    () => {
        None
    };
    ($locked:ident) => {
        Some(stringify!($locked))
    };
}

/// Generate the [`TrackListColumnState`] and [`TrackListActions`] impls for every listed Slint
/// global, plus [`hydrate_all`] and [`snapshot_all`] over the same list. `hidden` names the
/// toggleable columns and marks a first launch hides, and a detail view's `locked = <column>` is
/// excluded from the persisted set and forced off on hydrate.
macro_rules! track_list_views {
    ($(
        $global:ident => $view_id:ident, hidden = [$($hidden:ident),*] $(, locked = $locked:ident)?;
    )*) => {
        $(
            impl TrackListColumnState for $global<'_> {
                const VIEW_ID: &'static str = view_id::$view_id;
                const HIDDEN_BY_DEFAULT: &'static [&'static str] = &[$(stringify!($hidden)),*];
                const LOCKED: Option<&'static str> = locked_column!($($locked)?);

                fn read_columns(&self) -> TrackColumns {
                    self.get_track_columns()
                }

                fn write_columns(&self, columns: TrackColumns) {
                    self.set_track_columns(columns);
                }
            }

            impl TrackListActions for $global<'_> {
                fn bind_play_next(&self, f: impl FnMut(ModelRc<i32>) + 'static) {
                    self.on_play_next(f);
                }

                fn bind_add_to_queue(&self, f: impl FnMut(ModelRc<i32>) + 'static) {
                    self.on_add_to_queue(f);
                }

                fn bind_toggle_column(&self, f: impl FnMut(SharedString) + 'static) {
                    self.on_toggle_column(f);
                }
            }
        )*

        /// Hydrate every track list's column widths and visibility from `views.json`.
        pub fn hydrate_all(app: &AppWindow, vs: &ViewStateData) {
            $(hydrate(vs, &app.global::<$global>());)*
        }

        /// Snapshot every track list's column widths and visibility into `vs`. The caller writes
        /// it back to disk.
        pub fn snapshot_all(app: &AppWindow, vs: &mut ViewStateData) {
            $(snapshot(vs, &app.global::<$global>());)*
        }
    };
}

track_list_views! {
    Tracks => TRACKS, hidden = [];
    Browse => BROWSE, hidden = [];
    AlbumDetail => ALBUM_DETAIL, hidden = [], locked = album;
    // The number column is off on the two details whose tracks span albums, where a row's position
    // in the list is incidental rather than a tracklist position.
    ArtistDetail => ARTIST_DETAIL, hidden = [number], locked = artist;
    GenreDetail => GENRE_DETAIL, hidden = [number], locked = genre;
    PlaylistDetail => PLAYLIST_DETAIL, hidden = [genre];
    // Every row there is a favourite, so the mark says nothing; the hover heart still unfavourites.
    Favorites => FAVORITES, hidden = [number, genre, year, favorite_mark];
    RecentlyPlayed => RECENTLY_PLAYED, hidden = [number, genre, year];
    Search => SEARCH, hidden = [genre, year];
}

/// Narrow a persisted f64 column width (`views.json`) to the f32 Slint uses.
/// Column widths are tens-to-hundreds of pixels; f32 has ample precision.
#[allow(
    clippy::cast_possible_truncation,
    reason = "column widths are small pixel counts; f32 mantissa is sufficient"
)]
fn px_to_slint(v: f64) -> f32 {
    v as f32
}

#[cfg(test)]
#[path = "tests/track_list_view_tests.rs"]
mod tests;
