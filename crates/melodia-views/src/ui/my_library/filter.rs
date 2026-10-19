//! Where a keystroke in the page's single filter box lands, and where the box gets its
//! text back from when the surface under it changes.
//!
//! **The two halves of the page answer different filter contracts, which is why this
//! can't be one call.** A grid or list global fires `apply-filter(text)` and Rust ignores
//! the argument, the load reading `<Global>.filter` back as its query; a
//! detail global fires `filter-changed(text)` and Rust *uses* it. So the routing is a
//! match on (mounted tab, open detail), invoking each view's **existing** callback rather
//! than reaching for its `*Ui` handle — which keeps the whole input an `&AppWindow`.
//!
//! **Every arm still writes `<Global>.filter` first**, for reasons that differ by half. A
//! grid arm because the load reads it back; a detail arm because it has a live *Slint*
//! reader in `playlist-detail.slint`'s `reorder-enabled`, which refuses a drag while the
//! list is filtered — left unwritten, a filtered playlist stays reorderable and the
//! index → position mapping the drag depends on is wrong.
//!
//! The band's box is the only user-facing writer of `MyLibrary.filter`, and no `.slint`
//! element can declare a binding on another global's property, so both directions of the
//! hand-off are writes: [`dispatch`] outward, [`sync_box`] back.

use slint::{ComponentHandle, SharedString};

use super::{MountedSurface, MyLibraryTab, mounted_surface};
use melodia_ui::{
    AlbumDetail, Albums, AppWindow, ArtistDetail, Artists, GenreDetail, Genres, MyLibrary,
    PlaylistDetail, Playlists, Tracks,
};

/// Run a body against whichever global `$surface` names.
///
/// The five tabs and their four details are nine distinct Slint-generated types, and a
/// `match` whose arms bind different types stays a macro whatever traits those types
/// share; everything each arm then does is identical, which is what the callback body
/// absorbs. Two callbacks rather than one, for the module doc's contract split.
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
/// A tab that is genuinely filtered has to be cleared — a Songs needle carried into the
/// Albums grid would silently hide cards — and one that isn't is left alone, a dispatch
/// costing Songs a requery for nothing. A grid tab's own requery waits for its enter's
/// reload, the pick running ahead of the section gate.
pub fn clear_mounted(ui: &AppWindow) {
    if mounted_filter(ui).is_empty() {
        return;
    }
    dispatch(ui, "");
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
