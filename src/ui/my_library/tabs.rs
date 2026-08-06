//! Which My Library sub-view is mounted, and how that answer is seeded.
//!
//! The indices themselves live in `my-library.slint`'s `tab-*` constants — no Rust file
//! restates them. [`tab_from_index`] resolves one on the UI thread, which is the only
//! place the global is reachable and, unlike the two curated pages, the only place any
//! caller needs it: the five views each carry their own `section_active` shadow.

use slint::ComponentHandle;

use crate::{
    AlbumDetail, AppWindow, ArtistDetail, GenreDetail, MyLibrary, Nav, PlaylistDetail,
};

/// The tab index a section without tabs answers with. Only My Library has any.
///
/// Lives here rather than beside either reader: `nav_history` and
/// `callbacks::cross_tab_nav` each declared their own `-1` with the same meaning, and
/// the two are compared against each other every time a history entry is replayed.
pub const NO_TAB: i32 = -1;

/// The tab `section` currently has mounted, or [`NO_TAB`] for a section without tabs.
///
/// The one place the "is this the tabbed page, and if so which tab" question is
/// answered. `nav_history` asks it about a recorded entry's section and
/// `cross_tab_nav::Origin::read` about the live one; spelled twice, the two could
/// disagree about what a tabless section reports and a replay would then never match
/// its own recording.
pub fn tab_of_section(ui: &AppWindow, section: i32) -> i32 {
    if section == super::NAV_MY_LIBRARY {
        ui.global::<MyLibrary>().get_tab_idx()
    } else {
        NO_TAB
    }
}

/// Move the My Library tab and remember it.
///
/// The `Nav.selected-index` / `persist-selected-index` pair one level down, and
/// deliberately **not** `tab-changed`, which is a *pick* and clears the shared filter
/// box. A no-op for [`NO_TAB`], i.e. for every section that has no tabs — which is what
/// lets a caller hand it a recorded entry's tab without testing the section first.
pub fn persist_tab(ui: &AppWindow, tab: i32) {
    if tab < 0 {
        return;
    }
    let g = ui.global::<MyLibrary>();
    g.set_tab_idx(tab);
    g.invoke_persist_tab_idx(tab);
}

/// Which My Library sub-view is mounted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MyLibraryTab {
    Songs,
    Albums,
    Artists,
    Genres,
    Playlists,
}

/// Resolve a `MyLibrary.tab-idx` value against the global's own `tab-*` constants. UI
/// thread only — that's where the global is reachable.
pub fn tab_from_index(g: &MyLibrary<'_>, idx: i32) -> MyLibraryTab {
    if idx == g.get_tab_albums() {
        MyLibraryTab::Albums
    } else if idx == g.get_tab_artists() {
        MyLibraryTab::Artists
    } else if idx == g.get_tab_genres() {
        MyLibraryTab::Genres
    } else if idx == g.get_tab_playlists() {
        MyLibraryTab::Playlists
    } else {
        MyLibraryTab::Songs
    }
}

/// Whether `tab` is the sub-view on screen right now.
///
/// **The wire-time seed for all five views' `section_active` shadows**, written once
/// here rather than five times: `SectionActiveGate` fires on transitions only and its
/// `ChangeTracker` baselines silently inside `AppWindow::new()`, so a shadow seeded
/// against the nav index alone answers `true` for every tab and the four that aren't
/// mounted never get an edge to correct them. What that costs is in
/// `.claude/rules/ui-patterns.md`'s `SectionActiveGate` bullet.
pub fn tab_is_mounted(ui: &AppWindow, tab: MyLibraryTab) -> bool {
    if ui.global::<Nav>().get_selected_index() != super::NAV_MY_LIBRARY {
        return false;
    }
    let g = ui.global::<MyLibrary>();
    tab_from_index(&g, g.get_tab_idx()) == tab
}

/// Close whatever detail `tab` has open, if any.
///
/// The band's one back arrow and a Mouse-4 step out of a detail are the same act, so they
/// share the dispatch rather than each spelling the five arms: what each of the four
/// `close-detail` handlers then does — the hero images, the cover tiers,
/// `last_detail_ids`, the origin restore, the nav-history record — is unchanged and stays
/// where it is. Songs has no detail, so it is the no-op arm.
pub fn close_open_detail(ui: &AppWindow, tab: MyLibraryTab) {
    match tab {
        MyLibraryTab::Songs => {}
        MyLibraryTab::Albums => ui.global::<AlbumDetail>().invoke_close_detail(),
        MyLibraryTab::Artists => ui.global::<ArtistDetail>().invoke_close_detail(),
        MyLibraryTab::Genres => ui.global::<GenreDetail>().invoke_close_detail(),
        MyLibraryTab::Playlists => ui.global::<PlaylistDetail>().invoke_close_detail(),
    }
}

/// Seed the active tab from `views.json`, clamped against the Slint-declared `tab-count`
/// (see [`crate::ui::tab_bar::clamp_tab`]).
///
/// **Called from `install_views` before `wire_all`, beside the nav-index hydration**, and
/// that ordering is the whole point: each of the five views seeds its `section_active`
/// shadow at wire time from `Nav.selected-index == 3 && MyLibrary.tab-idx == <its tab>`.
/// Seed afterwards and every one of them answers for the global's declared `0`, so Songs
/// comes up active and the persisted tab inactive — and `SectionActiveGate`'s
/// `ChangeTracker` baselines silently inside `AppWindow::new()`, so there is no edge left
/// to correct it. The visible cost is a full Tracks query on every launch that resumes on
/// another tab, with the real tab's fetch landing late behind it.
///
/// Stateless, unlike the two curated pages' `seed_tab` — there is no handle to seed. See
/// the module docs.
pub fn seed_tab(ui: &AppWindow, persisted_tab: i32) {
    let g = ui.global::<MyLibrary>();
    g.set_tab_idx(crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count()));
}

/// Return to the view a drill started from: the `origin-nav-index` / `origin-tab` pair
/// a detail global carries, restored together.
///
/// Called from each detail's `on_close_detail`, **before** it clears its own id, so
/// Slint reroutes straight to the origin in one frame rather than flashing the grid
/// this detail sits over. Restoring the nav index alone used to be the whole job; with
/// five views sharing index 3 it is a no-op that leaves the wrong tab mounted.
pub fn restore_origin(ui: &AppWindow, origin_nav: i32, origin_tab: i32) {
    if origin_nav == super::NAV_MY_LIBRARY {
        persist_tab(ui, origin_tab);
    }
    let nav = ui.global::<Nav>();
    nav.set_selected_index(origin_nav);
    nav.invoke_persist_selected_index(origin_nav);
}
