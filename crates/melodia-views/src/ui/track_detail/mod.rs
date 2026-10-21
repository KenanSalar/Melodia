//! What the four track-list details (Album, Artist, Genre and Playlist) share: the Slint surface
//! of their globals, the view handle the shared wiring drives, the list each keeps as a keyset
//! (a [`PagedTrackList`] through the one impl below), and the launch's reopen. The wiring itself
//! is [`crate::ui::callbacks::track_detail`].

mod cache;
pub mod load;
pub mod restore;

pub use cache::DetailCache;

use std::sync::Arc;

use slint::{ModelRc, SharedString};

use crate::ui::list_selection::RowSelectionView;
use crate::ui::my_library::MyLibraryTab;
use crate::ui::paged_rows::track_rows::{ListLoads, ListQuery, PagedTrackList, TrackRows};
use crate::ui::section_state::SectionState;
use crate::ui::track_list_view::{TrackListActions, TrackListColumnState};
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::AppResult;
use melodia_ui::{AlbumDetail, ArtistDetail, GenreDetail, PlaylistDetail};

/// The Slint surface of a detail global the shared wiring needs beyond what every track list has.
/// Method names stay apart from the generated accessors, as [`RowSelectionView`]'s do.
pub trait DetailGlobal: RowSelectionView + TrackListColumnState + TrackListActions {
    /// The current `(sort-field, sort-dir)`.
    fn sort(&self) -> (SharedString, SharedString);
    fn set_sort(&self, field: &str, dir: &str);
    /// Writes the entity id whose `>= 0` mounts the detail over the grid.
    fn set_detail_id(&self, id: i32);
    fn clear_filter(&self);

    fn bind_shuffle(&self, f: impl FnMut() + 'static);
    fn bind_play_row(&self, f: impl FnMut(i32, i32) + 'static);
    fn bind_toggle_row_favorite(&self, f: impl FnMut(ModelRc<i32>, bool) + 'static);
    fn bind_set_row_rating(&self, f: impl FnMut(ModelRc<i32>, i32) + 'static);
    fn bind_request_sort(&self, f: impl FnMut(SharedString) + 'static);
    fn bind_filter_changed(&self, f: impl FnMut(SharedString) + 'static);
}

/// Generate a [`DetailGlobal`] impl. The id setter and the shuffle callback are the two
/// accessors whose names differ between the globals.
macro_rules! impl_detail_global {
    ($global:ident, $set_id:ident, $on_shuffle:ident) => {
        impl DetailGlobal for $global<'_> {
            fn sort(&self) -> (SharedString, SharedString) {
                (self.get_sort_field(), self.get_sort_dir())
            }
            fn set_sort(&self, field: &str, dir: &str) {
                self.set_sort_field(SharedString::from(field));
                self.set_sort_dir(SharedString::from(dir));
            }
            fn set_detail_id(&self, id: i32) {
                self.$set_id(id);
            }
            fn clear_filter(&self) {
                self.set_filter(SharedString::default());
            }
            fn bind_shuffle(&self, f: impl FnMut() + 'static) {
                self.$on_shuffle(f);
            }
            fn bind_play_row(&self, f: impl FnMut(i32, i32) + 'static) {
                self.on_play_row(f);
            }
            fn bind_toggle_row_favorite(&self, f: impl FnMut(ModelRc<i32>, bool) + 'static) {
                self.on_toggle_row_favorite(f);
            }
            fn bind_set_row_rating(&self, f: impl FnMut(ModelRc<i32>, i32) + 'static) {
                self.on_set_row_rating(f);
            }
            fn bind_request_sort(&self, f: impl FnMut(SharedString) + 'static) {
                self.on_request_sort(f);
            }
            fn bind_filter_changed(&self, f: impl FnMut(SharedString) + 'static) {
                self.on_filter_changed(f);
            }
        }
    };
}

impl_detail_global!(AlbumDetail, set_album_id, on_shuffle_album);
impl_detail_global!(ArtistDetail, set_artist_id, on_shuffle_artist);
impl_detail_global!(GenreDetail, set_genre_id, on_shuffle_genre);
impl_detail_global!(PlaylistDetail, set_playlist_id, on_shuffle_all);

/// A detail view's handle, as the shared wiring drives it.
///
/// `Global` is the `'static` form, which is what `slint::Global::as_weak` hands back, so the
/// trait methods taking one are for the wiring. A view's own UI-thread code holds a borrowed
/// global and calls the functions these forward to.
pub trait TrackDetail: Send + Sync + 'static {
    type Global: DetailGlobal + slint::StrongHandle + 'static;

    /// The key `views.json` holds this view under, and the prefix of its log lines.
    const VIEW_ID: &'static str = <Self::Global as TrackListColumnState>::VIEW_ID;
    /// The order a third click on a sorted header returns to, for a view whose own order is one
    /// no header names. `None` cycles between the two directions.
    const NATURAL_SORT: Option<&'static str> = None;
    /// The My Library tab the detail opens over.
    const TAB: MyLibraryTab;

    fn cache(&self) -> &DetailCache<Self::Global>;

    fn section(&self) -> &SectionState;

    /// The row tier the rows a load opens on are warmed into.
    fn row_covers(&self) -> &Arc<CoverThumbs>;

    /// The tracks the detail of entity `id` lists.
    fn scope(&self, id: i64) -> TrackListScope;

    fn detail_open(&self) -> bool {
        self.cache().id() >= 0
    }

    /// Narrows whatever else on the page the filter box describes. Artist Detail's Albums strip
    /// is the one.
    fn refilter_beside(&self, _g: &Self::Global) {}

    /// A load is about to show ids other than the ones on screen, so anything keyed on a row index
    /// is stale. Playlist Detail's drag is the one.
    fn rows_moving(&self, _g: &Self::Global) {}

    /// Drops the detail's Rust-side state once its id is back to `-1`.
    fn clear_detail(&self) {
        self.cache().clear();
    }
}

// The hooks a detail spells on `TrackDetail` keep their names here, so the bodies name the trait
// they forward to.
impl<V: TrackDetail> PagedTrackList for V {
    type Global = V::Global;
    type Facts = ();

    const LABEL: &'static str = V::VIEW_ID;

    fn loads(&self) -> &ListLoads {
        self.cache().loads()
    }

    fn global(&self) -> &slint::Weak<V::Global> {
        self.cache().global()
    }

    fn row_covers(&self) -> &Arc<CoverThumbs> {
        TrackDetail::row_covers(self)
    }

    fn query(&self) -> Option<ListQuery> {
        let (id, order, needle) = self.cache().query();
        (id >= 0).then(|| ListQuery { scope: self.scope(id), order, needle })
    }

    fn facts(_: &AppState, _: &ListQuery, _: &[i64]) -> impl Future<Output = AppResult<()>> + Send {
        std::future::ready(Ok(()))
    }

    /// The detail still lists the entity the load was for.
    fn still_wanted(&self, query: &ListQuery) -> bool {
        self.scope(self.cache().id()) == query.scope
    }

    fn rows_moving(&self, g: &V::Global) {
        TrackDetail::rows_moving(self, g);
    }

    fn shown(&self, _: &V::Global, query: &ListQuery, _: Option<()>) {
        let cache = self.cache();
        cache.record_shown(cache.id(), query.order, &query.needle);
    }
}

/// The ids the detail shows, in order, for a play or a shuffle.
pub fn displayed_ids<G: DetailGlobal>(g: &G) -> Vec<i64> {
    TrackRows::ids_of(&g.track_rows(), G::VIEW_ID)
}

/// Empty the detail's list, its ids and cached rows with it, and refuse every load still on its
/// way to it. For a close and a section leave. UI thread.
pub fn release_rows<V: TrackDetail>(view: &V, g: &impl DetailGlobal) {
    view.cache().loads().tickets.revoke();
    view.cache().forget_shown();
    let rows = g.track_rows();
    if let Some(model) = TrackRows::installed(&rows, V::VIEW_ID) {
        model.clear();
    }
}
