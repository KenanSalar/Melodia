//! What the four track-list details (Album, Artist, Genre and Playlist) share: the Slint surface
//! of their globals, the view handle the shared wiring drives, and the Rust-side cache. The
//! wiring itself is [`crate::ui::callbacks::track_detail`].

mod cache;
pub mod filter;
pub mod selection;

pub use cache::DetailCache;

use std::sync::Arc;

use slint::{ModelRc, SharedString};

use crate::ui::list_selection::RowSelectionView;
use crate::ui::track_list_view::{TrackListActions, TrackListColumnState};
use melodia_ui::{
    AlbumDetail, ArtistDetail, GenreDetail, PlaylistDetail, TrackListRow as UiTrackListRow,
};

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
    fn bind_select_row(&self, f: impl FnMut(i32, i32, bool, bool) + 'static);
    fn bind_select_all(&self, f: impl FnMut() + 'static);
    fn bind_clear_selection(&self, f: impl FnMut() + 'static);
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
            fn bind_select_row(&self, f: impl FnMut(i32, i32, bool, bool) + 'static) {
                self.on_select_row(f);
            }
            fn bind_select_all(&self, f: impl FnMut() + 'static) {
                self.on_select_all(f);
            }
            fn bind_clear_selection(&self, f: impl FnMut() + 'static) {
                self.on_clear_selection(f);
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

    fn cache(&self) -> &DetailCache;

    fn resort(&self, g: &Self::Global) {
        self.cache().resort(g);
    }

    fn refilter(&self, g: &Self::Global) {
        filter::apply_filtered_detail(g, self.cache());
    }

    /// Drops the detail's Rust-side state once its id is back to `-1`.
    fn clear_detail(&self) {
        self.cache().clear();
    }
}

/// A detail's cached rows and its model, for a heart or a star landing on a track it may list.
/// Neither changes list membership, so both patch in place: no re-fetch, and the scroll holds.
pub struct DetailRows<V: TrackDetail> {
    view: Arc<V>,
    global: slint::Weak<V::Global>,
}

// By hand, a derive bounding `V: Clone` where only the `Arc` is cloned.
impl<V: TrackDetail> Clone for DetailRows<V> {
    fn clone(&self) -> Self {
        Self { view: self.view.clone(), global: self.global.clone() }
    }
}

impl<V: TrackDetail> DetailRows<V> {
    pub fn new(view: &Arc<V>, global: slint::Weak<V::Global>) -> Self {
        Self { view: view.clone(), global }
    }

    /// A no-op where the track isn't listed. Safe from any thread.
    pub fn mirror_favorite(&self, id: i64, fav: bool) {
        self.view.cache().flip_favorite(id, fav);
        self.patch_row(id, move |r| r.is_favorite = fav);
    }

    /// [`Self::mirror_favorite`]'s star-rating twin.
    pub fn mirror_rating(&self, id: i64, rating: i32) {
        self.view.cache().flip_rating(id, rating);
        self.patch_row(id, move |r| r.rating = rating);
    }

    fn patch_row(&self, id: i64, patch: impl Fn(&mut UiTrackListRow) + Send + 'static) {
        let _ = self.global.upgrade_in_event_loop(move |g| {
            crate::ui::model_patch::patch_track_row_by_id(&g.track_rows(), id, patch);
        });
    }
}
