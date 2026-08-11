//! Where a keystroke in the page's single filter box lands, and where the box gets its
//! text back from when the surface under it changes.
//!
//! **The two halves of the page answer different filter contracts, which is why this
//! can't be one call.** A grid or list global fires `apply-filter(text)` and Rust ignores
//! the argument — `albums/callbacks/grid.rs` is `on_apply_filter(move |_text| …
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

use super::{MountedSurface, MyLibraryTab, mounted_surface};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::{
    AlbumDetail, Albums, AppWindow, ArtistDetail, Artists, GenreDetail, Genres, MyLibrary,
    PlaylistDetail, Playlists, Tracks,
};

/// Run `$body` against whichever global `$surface` names, binding it as `$g`.
///
/// The five tabs and their four details are nine distinct Slint-generated types with no
/// trait between them, so the *dispatch* can't be a function — but everything each arm
/// then does is identical, which is what the callback body absorbs. Same wall, and same
/// answer, as `impl_mosaic_hero!` and `impl_detail_view_helpers`.
///
/// Two callbacks are threaded through rather than one because the halves of the page
/// answer different contracts and always have: a grid or list global fires
/// `apply-filter(text)` and Rust ignores the argument, reading `<Global>.filter` back
/// inside a memoized rebuild, where a detail global fires `filter-changed(text)` and Rust
/// uses it. `$grid` runs on the five grid/list surfaces, `$detail` on the four details.
macro_rules! on_mounted_surface {
    ($ui:expr, $surface:expr, |$g:ident| $grid:expr, |$d:ident| $detail:expr $(,)?) => {{
        let ui = $ui;
        let surface: MountedSurface = $surface;
        match surface.tab {
            // Songs has no detail view; its list is the only surface.
            MyLibraryTab::Songs => {
                let $g = ui.global::<Tracks>();
                $grid
            }
            MyLibraryTab::Albums if surface.detail_open() => {
                let $d = ui.global::<AlbumDetail>();
                $detail
            }
            MyLibraryTab::Albums => {
                let $g = ui.global::<Albums>();
                $grid
            }
            MyLibraryTab::Artists if surface.detail_open() => {
                let $d = ui.global::<ArtistDetail>();
                $detail
            }
            MyLibraryTab::Artists => {
                let $g = ui.global::<Artists>();
                $grid
            }
            MyLibraryTab::Genres if surface.detail_open() => {
                let $d = ui.global::<GenreDetail>();
                $detail
            }
            MyLibraryTab::Genres => {
                let $g = ui.global::<Genres>();
                $grid
            }
            MyLibraryTab::Playlists if surface.detail_open() => {
                let $d = ui.global::<PlaylistDetail>();
                $detail
            }
            MyLibraryTab::Playlists => {
                let $g = ui.global::<Playlists>();
                $grid
            }
        }
    }};
}

/// Route the page's filter text to whichever surface the mounted tab is showing.
pub fn dispatch(ui: &AppWindow, text: &str) {
    let needle = SharedString::from(text);
    on_mounted_surface!(
        ui,
        mounted_surface(ui),
        |g| {
            g.set_filter(needle.clone());
            g.invoke_apply_filter(needle);
        },
        |d| {
            d.set_filter(needle.clone());
            d.invoke_filter_changed(needle);
        },
    );
}

/// Whichever of the nine surfaces is mounted, and what it is currently filtered by.
///
/// [`dispatch`] read for its answer instead of its destination — the same routing, so the
/// two halves of the hand-off cannot drift apart.
fn mounted_filter(ui: &AppWindow) -> SharedString {
    on_mounted_surface!(ui, mounted_surface(ui), |g| g.get_filter(), |d| d.get_filter())
}

/// Drop the entering tab's own needle, if it has one.
///
/// **The guard is the point, not an optimization.** A tab pick reaches a surface whose
/// section leave has already wiped its Rust cache, so dispatching into it rebuilds from
/// nothing — and for the four grid tabs that write is `total-count = 0` plus an empty
/// model, which is exactly the pair their `GridEmptyState` mounts on. The leave had set
/// the count to [`crate::ui::tab_bar::UNFETCHED_COUNT`] so the panel would stay quiet
/// until the fetch answered it; dispatching unconditionally overwrote that with the one
/// value that means "there is nothing here", and the tab came up asserting an empty
/// library for as long as its query took. Songs is the same write one surface over: its
/// model survives the leave, so it pays a second full-library filter walk on the event
/// loop instead of painting a lie.
///
/// A tab that is genuinely filtered still has to be cleared — a Songs needle carried into
/// the Albums grid would silently hide cards — and there the rebuild is what makes the
/// clear visible, so it is dispatched as before. That rebuild writes the same `0` the guard
/// exists to avoid, which is why [`rewind_grid_count`] follows it: the guard removes the
/// *common* case, and the rewind covers what is left of the rare one.
pub fn clear_mounted(ui: &AppWindow) {
    if mounted_filter(ui).is_empty() {
        return;
    }
    dispatch(ui, "");
    rewind_grid_count(ui);
}

/// Put an entering grid tab's `total-count` back to the sentinel after a dispatch.
///
/// The dispatch above rebuilt from the cache that tab's own section leave wiped, so the
/// count it wrote is `0` — "there is nothing here" — where the truth is "not fetched yet".
/// The leave had set [`UNFETCHED_COUNT`] for exactly that reason and marked the view dirty,
/// so the gate's re-fetch is already scheduled and is what answers this; all that is owed
/// here is not to have overwritten the sentinel in between.
///
/// **Songs and the four details are excluded, and neither is an omission.** Songs' model
/// survives the leave, so `refilter` re-derives the full count off a warm cache and the
/// number was never a lie. A detail arm writes no grid count at all — its own `open_*`
/// re-runs on the same section enter.
fn rewind_grid_count(ui: &AppWindow) {
    let surface = mounted_surface(ui);
    // Songs falls in the grid arm too and must not rewind, so it is excluded here rather
    // than by the dispatch: `Tracks` carries no `total-count` for the header to read.
    if surface.tab == MyLibraryTab::Songs {
        return;
    }
    on_mounted_surface!(
        ui,
        surface,
        |g| g.set_total_count(UNFETCHED_COUNT),
        // A detail arm writes no grid count at all — its own `open_*` re-runs on the same
        // section enter.
        |_d| (),
    );
}

/// Reseat the page's box from whichever surface is now mounted.
///
/// [`dispatch`] backwards. The sheet calls it for the two things that swap the surface
/// under a box nobody typed in — a detail id crossing zero, and a tab move that isn't a
/// pick — and neither is the tab pick's clear-both-sides rule.
///
/// On an **id**, both directions matter: **a drill-in** finds the detail's own filter
/// already cleared by `open_*`, so the box empties, where leaving it would show the grid's
/// needle over a list it filters nothing of; **a back out** finds the grid's needle still
/// there and untouched — the grid rebuild is memoized on it, so the cards come back
/// filtered and the box has to say so. Clearing on the way out instead would drop the
/// user's grid filter on every back, which is the one thing the retired per-view boxes
/// never did.
///
/// On a **tab**, the asymmetry in [`dispatch`] is what makes it necessary: a pick clears
/// only the *entering* tab's needle, so the departing one keeps its own, and the two
/// arrivals that aren't picks — a cross-tab drill, a Mouse-4/5 walk, both through
/// `persist-tab-idx` — land on a tab still filtered by a needle nothing touched. A pick
/// reaches here too and finds both sides already empty, so it bails below.
pub fn sync_box(ui: &AppWindow) {
    let mounted = mounted_filter(ui);
    let g = ui.global::<MyLibrary>();
    if g.get_filter() == mounted {
        return;
    }
    g.set_filter(mounted);
    // The box now says something the user didn't type, so it has no business holding the
    // keyboard — the same tick every backdrop click bumps.
    g.set_blur_search_tick(g.get_blur_search_tick() + 1);
}
