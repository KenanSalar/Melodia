//! Where a keystroke in the page's single filter box lands.
//!
//! **The two halves of the page answer different filter contracts, which is why this
//! can't be one call.** A grid or list global fires `apply-filter(text)` and Rust ignores
//! the argument — `callbacks/albums/grid.rs` is `on_apply_filter(move |_text| …
//! rebuild_grid(…))` and the rebuild reads `<Global>.filter` itself, memoized against a
//! `GridIndexCache`. A detail global fires `filter-changed(text)` and Rust *uses* the
//! argument, folding it into that view's `Mutex<Needle>`. So the routing is a match on
//! (mounted tab, open detail), and it invokes each view's **existing** callback rather
//! than reaching for its `*Ui` handle — which is what keeps this function's whole input
//! an `&AppWindow`.
//!
//! Nothing writes `MyLibrary.filter` yet: the five views still carry their own search
//! boxes until the band replaces them. The routing lands here now because the band is
//! what supplies the writer, not what decides where the text goes.

use slint::{ComponentHandle, SharedString};

use super::{MyLibraryTab, tab_from_index};
use crate::{
    AlbumDetail, Albums, AppWindow, ArtistDetail, Artists, GenreDetail, Genres, MyLibrary,
    PlaylistDetail, Playlists, Tracks,
};

/// Route the page's filter text to whichever surface the mounted tab is showing.
pub fn dispatch(ui: &AppWindow, text: &str) {
    let g = ui.global::<MyLibrary>();
    let needle = SharedString::from(text);

    match tab_from_index(&g, g.get_tab_idx()) {
        // Songs has no detail view; its list is the only surface.
        MyLibraryTab::Songs => ui.global::<Tracks>().invoke_apply_filter(needle),
        MyLibraryTab::Albums => {
            let detail = ui.global::<AlbumDetail>();
            if detail.get_album_id() >= 0 {
                detail.invoke_filter_changed(needle);
            } else {
                ui.global::<Albums>().invoke_apply_filter(needle);
            }
        }
        MyLibraryTab::Artists => {
            let detail = ui.global::<ArtistDetail>();
            if detail.get_artist_id() >= 0 {
                detail.invoke_filter_changed(needle);
            } else {
                ui.global::<Artists>().invoke_apply_filter(needle);
            }
        }
        MyLibraryTab::Genres => {
            let detail = ui.global::<GenreDetail>();
            if detail.get_genre_id() >= 0 {
                detail.invoke_filter_changed(needle);
            } else {
                ui.global::<Genres>().invoke_apply_filter(needle);
            }
        }
        MyLibraryTab::Playlists => {
            let detail = ui.global::<PlaylistDetail>();
            if detail.get_playlist_id() >= 0 {
                detail.invoke_filter_changed(needle);
            } else {
                ui.global::<Playlists>().invoke_apply_filter(needle);
            }
        }
    }
}
