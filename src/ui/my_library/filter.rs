//! Where a keystroke in the page's single filter box lands, and where the box gets its
//! text back from when the surface under it changes.
//!
//! **The two halves of the page answer different filter contracts, which is why this
//! can't be one call.** A grid or list global fires `apply-filter(text)` and Rust ignores
//! the argument — `callbacks/albums/grid.rs` is `on_apply_filter(move |_text| …
//! rebuild_grid(…))` and the rebuild reads `<Global>.filter` itself, memoized against a
//! `GridIndexCache`. A detail global fires `filter-changed(text)` and Rust *uses* the
//! argument, folding it into that view's `Mutex<Needle>`. So the routing is a match on
//! (mounted tab, open detail), and it invokes each view's **existing** callback rather
//! than reaching for its `*Ui` handle — which is what keeps these functions' whole input
//! an `&AppWindow`.
//!
//! **Every arm still writes `<Global>.filter` first**, and the reasons differ by half. A
//! grid arm has to: the rebuild reads the property back rather than taking the argument.
//! A detail arm has to for a reason that only appeared when the detail views lost their
//! own search boxes — the property was theirs to keep current through a `<=>`, and it has
//! a live reader in `playlist-detail.slint`'s `reorder-enabled`, which refuses a drag
//! while the list is filtered. Left unwritten, a filtered playlist stays reorderable and
//! the index → position mapping the drag depends on is wrong. So the nine arms now read
//! alike, and the property is simply the box's mirror onto whichever surface is mounted.
//!
//! The band's box is the only *user-facing* writer of `MyLibrary.filter`, and no `.slint`
//! element can declare a binding on another global's property — bindings belong to the
//! scope they are written in — so both directions of the hand-off are writes: [`dispatch`]
//! outward, [`sync_box`] back.

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
        MyLibraryTab::Songs => {
            let tracks = ui.global::<Tracks>();
            tracks.set_filter(needle.clone());
            tracks.invoke_apply_filter(needle);
        }
        MyLibraryTab::Albums => {
            let detail = ui.global::<AlbumDetail>();
            if detail.get_album_id() >= 0 {
                detail.set_filter(needle.clone());
                detail.invoke_filter_changed(needle);
            } else {
                let grid = ui.global::<Albums>();
                grid.set_filter(needle.clone());
                grid.invoke_apply_filter(needle);
            }
        }
        MyLibraryTab::Artists => {
            let detail = ui.global::<ArtistDetail>();
            if detail.get_artist_id() >= 0 {
                detail.set_filter(needle.clone());
                detail.invoke_filter_changed(needle);
            } else {
                let grid = ui.global::<Artists>();
                grid.set_filter(needle.clone());
                grid.invoke_apply_filter(needle);
            }
        }
        MyLibraryTab::Genres => {
            let detail = ui.global::<GenreDetail>();
            if detail.get_genre_id() >= 0 {
                detail.set_filter(needle.clone());
                detail.invoke_filter_changed(needle);
            } else {
                let grid = ui.global::<Genres>();
                grid.set_filter(needle.clone());
                grid.invoke_apply_filter(needle);
            }
        }
        MyLibraryTab::Playlists => {
            let detail = ui.global::<PlaylistDetail>();
            if detail.get_playlist_id() >= 0 {
                detail.set_filter(needle.clone());
                detail.invoke_filter_changed(needle);
            } else {
                let grid = ui.global::<Playlists>();
                grid.set_filter(needle.clone());
                grid.invoke_apply_filter(needle);
            }
        }
    }
}

/// Reseat the page's box from whichever surface is now mounted.
///
/// [`dispatch`] backwards, and the sheet calls it whenever a detail id crosses zero — the
/// one event that swaps the surface under a box nobody typed in. Both directions matter
/// and they are not the same rule as a tab pick's clear-both-sides: **a drill-in** finds
/// the detail's own filter already cleared by `open_*`, so the box empties, where leaving
/// it would show the grid's needle over a list it filters nothing of; **a back out** finds
/// the grid's needle still there and untouched — the grid rebuild is memoized on it, so
/// the cards come back filtered and the box has to say so. Clearing on the way out instead
/// would drop the user's grid filter on every back, which is the one thing the retired
/// per-view boxes never did.
pub fn sync_box(ui: &AppWindow) {
    let g = ui.global::<MyLibrary>();
    let mounted = match tab_from_index(&g, g.get_tab_idx()) {
        MyLibraryTab::Songs => ui.global::<Tracks>().get_filter(),
        MyLibraryTab::Albums => {
            let detail = ui.global::<AlbumDetail>();
            if detail.get_album_id() >= 0 {
                detail.get_filter()
            } else {
                ui.global::<Albums>().get_filter()
            }
        }
        MyLibraryTab::Artists => {
            let detail = ui.global::<ArtistDetail>();
            if detail.get_artist_id() >= 0 {
                detail.get_filter()
            } else {
                ui.global::<Artists>().get_filter()
            }
        }
        MyLibraryTab::Genres => {
            let detail = ui.global::<GenreDetail>();
            if detail.get_genre_id() >= 0 {
                detail.get_filter()
            } else {
                ui.global::<Genres>().get_filter()
            }
        }
        MyLibraryTab::Playlists => {
            let detail = ui.global::<PlaylistDetail>();
            if detail.get_playlist_id() >= 0 {
                detail.get_filter()
            } else {
                ui.global::<Playlists>().get_filter()
            }
        }
    };

    if g.get_filter() == mounted {
        return;
    }
    g.set_filter(mounted);
    // The box now says something the user didn't type, so it has no business holding the
    // keyboard — the same tick every backdrop click bumps.
    g.set_blur_search_tick(g.get_blur_search_tick() + 1);
}
