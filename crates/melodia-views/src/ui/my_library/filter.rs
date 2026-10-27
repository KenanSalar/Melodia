//! Where a keystroke in the page's one filter box lands, and where the box takes its text back
//! from when the surface under it changes. Both directions are writes, no `.slint` binding being
//! able to reach another global's property.
//!
//! **Grids and details answer different filter contracts.** A grid fires `apply-filter` and its
//! load reads `<Global>.filter` back, where a detail fires `filter-changed` and uses the argument;
//! each arm invokes the view's own callback, keeping the input an `&AppWindow`. Every arm writes
//! `<Global>.filter` first all the same: a detail has a Slint reader in `playlist-detail.slint`'s
//! `reorder-enabled`, and left unwritten a filtered playlist stays reorderable over the wrong
//! index mapping.

use slint::{ComponentHandle, SharedString};

use super::{MountedSurface, MyLibraryTab, mounted_surface};
use melodia_ui::{
    AlbumDetail, Albums, AppWindow, ArtistDetail, Artists, GenreDetail, Genres, MyLibrary,
    PlaylistDetail, Playlists, Tracks,
};

/// Runs a body against whichever global `$surface` names: nine distinct generated types, which
/// only a macro can bind in one `match`. Two bodies, for the module doc's contract split.
macro_rules! on_mounted_surface {
    ($ui:expr, $surface:expr, |$g:ident| $grid:expr, |$d:ident| $detail:expr $(,)?) => {{
        let ui = $ui;
        let surface: MountedSurface = $surface;
        match surface.tab {
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

/// Routes the page's filter text to whichever surface the mounted tab is showing.
pub fn dispatch(ui: &AppWindow, text: &str) {
    let needle = SharedString::from(text);
    // Once per settled burst, behind the view's `FilterThrottle`.
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

/// What the mounted surface is filtered by, through [`dispatch`]'s routing so the two can't drift.
fn mounted_filter(ui: &AppWindow) -> SharedString {
    on_mounted_surface!(ui, mounted_surface(ui), |g| g.get_filter(), |d| d.get_filter())
}

/// Drops the entering tab's own needle, and only when it has one, a dispatch costing Songs a
/// requery for nothing.
pub fn clear_mounted(ui: &AppWindow) {
    if mounted_filter(ui).is_empty() {
        return;
    }
    dispatch(ui, "");
}

/// Reseats the page's box from the surface now mounted, for the moves that swap it under a box
/// nobody typed in. A drill-in finds the detail's filter cleared by `open_*`, a back out finds
/// the grid's needle still applied, and a tab move that isn't a pick lands on a needle nothing
/// cleared. A pick reaches here too and finds both sides empty.
pub fn sync_box(ui: &AppWindow) {
    let mounted = mounted_filter(ui);
    let g = ui.global::<MyLibrary>();
    if g.get_filter() == mounted {
        return;
    }
    g.set_filter(mounted);
    // The box now says something the user didn't type, so it lets go of the keyboard.
    g.set_blur_search_tick(g.get_blur_search_tick() + 1);
}
