//! Where a keystroke in the page's single filter box lands, and where the box gets its
//! text back from when the surface under it changes.
//!
//! **The two halves of the page answer different filter contracts, which is why this
//! can't be one call.** A grid or list global fires `apply-filter(text)` and Rust ignores
//! the argument, the rebuild reading `<Global>.filter` back inside its own memoization; a
//! detail global fires `filter-changed(text)` and Rust *uses* it. So the routing is a
//! match on (mounted tab, open detail), invoking each view's **existing** callback rather
//! than reaching for its `*Ui` handle — which keeps the whole input an `&AppWindow`.
//!
//! **Every arm still writes `<Global>.filter` first**, for reasons that differ by half. A
//! grid arm because the rebuild reads it back; a detail arm because it has a live *Slint*
//! reader in `playlist-detail.slint`'s `reorder-enabled`, which refuses a drag while the
//! list is filtered — left unwritten, a filtered playlist stays reorderable and the
//! index → position mapping the drag depends on is wrong.
//!
//! The band's box is the only user-facing writer of `MyLibrary.filter`, and no `.slint`
//! element can declare a binding on another global's property, so both directions of the
//! hand-off are writes: [`dispatch`] outward, [`sync_box`] back.

use slint::{ComponentHandle, SharedString};

use super::{MountedSurface, MyLibraryTab, mounted_surface};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_ui::{
    AlbumDetail, Albums, AppWindow, ArtistDetail, Artists, GenreDetail, Genres, MyLibrary,
    PlaylistDetail, Playlists, Tracks,
};

/// Run a body against whichever global `$surface` names.
///
/// The five tabs and their four details are nine distinct Slint-generated types with no
/// trait between them, so the *dispatch* can't be a function; everything each arm then
/// does is identical, which is what the callback body absorbs. Two callbacks rather than
/// one, for the module doc's contract split.
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
    // Behind the view's `FilterThrottle`, so once per settled burst. Names the surface
    // too, one box driving nine.
    log::debug!("filter: {:?} → {:?}", text, mounted_surface(ui));
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
/// nothing — and on the four grid tabs that write is `total-count = 0` plus an empty
/// model, exactly the pair their `GridEmptyState` mounts on, overwriting the
/// [`crate::ui::tab_bar::UNFETCHED_COUNT`] the leave set to keep that panel quiet. Songs
/// pays a second full-library filter walk on the event loop instead, its model surviving.
///
/// A tab that is genuinely filtered still has to be cleared — a Songs needle carried into
/// the Albums grid would silently hide cards — and there the rebuild writes that same `0`,
/// which is what [`rewind_grid_count`] follows it for.
pub fn clear_mounted(ui: &AppWindow) {
    if mounted_filter(ui).is_empty() {
        return;
    }
    dispatch(ui, "");
    rewind_grid_count(ui);
}

/// Put an entering grid tab's `total-count` back to the sentinel after a dispatch, which
/// rebuilt from the cache that tab's own leave wiped and so wrote `0` — "there is nothing
/// here" — where the truth is "not fetched yet". The leave already marked the view dirty,
/// so all that is owed here is not to have overwritten [`UNFETCHED_COUNT`] in between.
///
/// **Songs and the four details are excluded, and neither is an omission.** Songs' model
/// survives the leave, so `refilter` re-derives a true count off a warm cache; a detail
/// arm writes no grid count at all, its own `open_*` re-running on the same enter.
fn rewind_grid_count(ui: &AppWindow) {
    let surface = mounted_surface(ui);
    // Songs falls in the grid arm too, so it is excluded here rather than by the
    // dispatch: `Tracks` carries no `total-count` for the header to read.
    if surface.tab == MyLibraryTab::Songs {
        return;
    }
    on_mounted_surface!(ui, surface, |g| g.set_total_count(UNFETCHED_COUNT), |_d| ());
}

/// Reseat the page's box from whichever surface is now mounted — [`dispatch`] backwards,
/// for the two things that swap the surface under a box nobody typed in.
///
/// On an **id**, both directions matter: a drill-in finds the detail's own filter already
/// cleared by `open_*`, so the box empties rather than showing the grid's needle over a
/// list it filters nothing of; a back out finds the grid's needle untouched and its
/// rebuild memoized on it, so the cards come back filtered and the box has to say so.
///
/// On a **tab**, [`dispatch`]'s asymmetry is what makes it necessary: a pick clears only
/// the *entering* tab's needle, so the arrivals that aren't picks — a cross-tab drill, a
/// Mouse-4/5 walk — land on a tab still filtered by a needle nothing touched. A pick
/// reaches here too and finds both sides empty, so it bails below.
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
