//! Which My Library tab is mounted. The indices are `my-library.slint`'s `tab-*` constants, which
//! no Rust file restates; no shadow is kept, each view carrying its own `section_active`.

use slint::ComponentHandle;

use melodia_ui::{
    AlbumDetail, AppWindow, ArtistDetail, GenreDetail, MyLibrary, Nav, PlaylistDetail,
};

pub const NO_TAB: i32 = -1;

/// The tab `section` has mounted, or [`NO_TAB`] for a section without tabs. One function for
/// `nav_history`'s recorded entries and `cross_tab_nav::Origin`'s live one, so a replay matches
/// its own recording.
pub fn tab_of_section(ui: &AppWindow, section: i32) -> i32 {
    if section == super::NAV_MY_LIBRARY { ui.global::<MyLibrary>().get_tab_idx() } else { NO_TAB }
}

/// Moves the tab and remembers it, deliberately not through `tab-changed`, a pick that clears the
/// shared filter box. A no-op for [`NO_TAB`], so a recorded entry's tab can be handed in as is.
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

impl MyLibraryTab {
    /// Every variant, pinned against `tab-count`: [`tab_from_index`] falls back to `Songs`, which
    /// `ui::view_tag` logs.
    pub const ALL: [Self; 5] =
        [Self::Songs, Self::Albums, Self::Artists, Self::Genres, Self::Playlists];
}

/// Resolves a `MyLibrary.tab-idx` value against the global's own `tab-*` constants.
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

/// Whether `tab` is on screen: the wire-time seed for the five views' `section_active` shadows.
/// The gate fires on transitions only, so a shadow seeded off the nav index alone answers `true`
/// on every tab, and the four not mounted never get an edge to correct it.
pub fn tab_is_mounted(ui: &AppWindow, tab: MyLibraryTab) -> bool {
    if ui.global::<Nav>().get_selected_index() != super::NAV_MY_LIBRARY {
        return false;
    }
    let g = ui.global::<MyLibrary>();
    tab_from_index(&g, g.get_tab_idx()) == tab
}

/// Whether a hero global is still reachable without a fetch: a tab switch clears no detail id, so
/// a detail held on another tab morphs back open on that tab's pick. A teardown inside this window
/// hands nothing back (`callbacks`' module doc). Deliberately not the section gate's predicate,
/// which also falls when Now Playing or the miniplayer covers the band: covering it is not leaving
/// it.
pub fn the_band_is_up(ui: &AppWindow) -> bool {
    ui.global::<Nav>().get_selected_index() == super::NAV_MY_LIBRARY
}

/// The detail `tab` has open, `None` on Songs. Asked per tab: a tab left with its detail open
/// keeps the id, so several can be set at once, and `nav_history` asks about a recorded entry's
/// tab, mounted or not.
pub fn detail_id_for(ui: &AppWindow, tab: MyLibraryTab) -> Option<i64> {
    let id = match tab {
        MyLibraryTab::Songs => return None,
        MyLibraryTab::Albums => i64::from(ui.global::<AlbumDetail>().get_album_id()),
        MyLibraryTab::Artists => i64::from(ui.global::<ArtistDetail>().get_artist_id()),
        MyLibraryTab::Genres => i64::from(ui.global::<GenreDetail>().get_genre_id()),
        MyLibraryTab::Playlists => i64::from(ui.global::<PlaylistDetail>().get_playlist_id()),
    };
    (id >= 0).then_some(id)
}

/// Which of the page's nine surfaces is on screen, the one answer for routing a keystroke,
/// reading a filter back, rewinding a count and resolving a history entry.
/// `hero_chips::my_library_owner` and `ui::view_tag::my_library_tag` stay off it, each asking a
/// different question of the same globals.
pub fn mounted_surface(ui: &AppWindow) -> MountedSurface {
    let tab = {
        let g = ui.global::<MyLibrary>();
        tab_from_index(&g, g.get_tab_idx())
    };
    MountedSurface { tab, detail_id: detail_id_for(ui, tab) }
}

/// The mounted tab and the detail it has open, from [`mounted_surface`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MountedSurface {
    pub tab: MyLibraryTab,
    /// `None` when the tab is showing its grid — or its list, on Songs.
    pub detail_id: Option<i64>,
}

impl MountedSurface {
    /// Whether a detail is covering the mounted tab's grid.
    #[must_use]
    pub fn detail_open(self) -> bool {
        self.detail_id.is_some()
    }
}

/// Closes whatever detail `tab` has open, the dispatch the band's back arrow and a Mouse-4 step
/// share.
pub fn close_open_detail(ui: &AppWindow, tab: MyLibraryTab) {
    match tab {
        MyLibraryTab::Songs => {}
        MyLibraryTab::Albums => ui.global::<AlbumDetail>().invoke_close_detail(),
        MyLibraryTab::Artists => ui.global::<ArtistDetail>().invoke_close_detail(),
        MyLibraryTab::Genres => ui.global::<GenreDetail>().invoke_close_detail(),
        MyLibraryTab::Playlists => ui.global::<PlaylistDetail>().invoke_close_detail(),
    }
}

/// Seeds the tab from `views.json`, clamped to `tab-count`. Called from `install_views` before
/// `wire_all`, beside the nav-index hydration, because each view seeds its `section_active`
/// shadow at wire time from both: seeded afterwards, every one answers for the declared `0` with
/// no edge left to correct it.
pub fn seed_tab(ui: &AppWindow, persisted_tab: i32) {
    let g = ui.global::<MyLibrary>();
    g.set_tab_idx(crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count()));
}

/// Lands on a tab from outside the bar, the destination half of a cross-section drill. The tab
/// goes first, so the page mounts on the body it is meant to show.
pub fn go_to_tab(ui: &AppWindow, tab: i32) {
    persist_tab(ui, tab);
    let nav = ui.global::<Nav>();
    nav.set_selected_index(super::NAV_MY_LIBRARY);
    nav.invoke_persist_selected_index(super::NAV_MY_LIBRARY);
}

/// Returns to the section a drill started from. Called before the detail clears its own id, so
/// Slint reroutes in one frame rather than flashing the grid; only a drill from another section
/// records an origin, the band's back arrow otherwise meaning "close this detail".
pub fn return_to_section(ui: &AppWindow, origin_nav: i32) {
    let nav = ui.global::<Nav>();
    nav.set_selected_index(origin_nav);
    nav.invoke_persist_selected_index(origin_nav);
}
