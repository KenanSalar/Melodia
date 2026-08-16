//! Which My Library sub-view is mounted, and how that answer is seeded.
//!
//! The indices live in `my-library.slint`'s `tab-*` constants and no Rust file restates
//! them. [`tab_from_index`] resolves one on the UI thread, the only place the global is
//! reachable and — unlike the two curated pages — the only place any caller needs it,
//! the five views each carrying their own `section_active` shadow.

use slint::ComponentHandle;

use crate::{AlbumDetail, AppWindow, ArtistDetail, GenreDetail, MyLibrary, Nav, PlaylistDetail};

/// The tab index a section without tabs answers with. Here rather than beside either
/// reader: `nav_history` and `callbacks::cross_tab_nav` are compared against each other
/// every time a history entry is replayed.
pub const NO_TAB: i32 = -1;

/// The tab `section` currently has mounted, or [`NO_TAB`] for a section without tabs.
///
/// `nav_history` asks it about a recorded entry's section and `cross_tab_nav::Origin`
/// about the live one; spelled twice, the two could disagree about what a tabless section
/// reports and a replay would never match its own recording.
pub fn tab_of_section(ui: &AppWindow, section: i32) -> i32 {
    if section == super::NAV_MY_LIBRARY {
        ui.global::<MyLibrary>().get_tab_idx()
    } else {
        NO_TAB
    }
}

/// Move the My Library tab and remember it. Deliberately **not** `tab-changed`, which is
/// a *pick* and clears the shared filter box. A no-op for [`NO_TAB`], so a caller can
/// hand it a recorded entry's tab without testing the section first.
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
    /// [`tab_from_index`] ends in a default arm, so a tab added to `my-library.slint`
    /// without one here resolves to `Songs` and `ui::view_tag` logs that. Pinned
    /// against `tab-count`.
    pub const ALL: [Self; 5] = [
        Self::Songs,
        Self::Albums,
        Self::Artists,
        Self::Genres,
        Self::Playlists,
    ];
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

/// Whether `tab` is the sub-view on screen right now — **the wire-time seed for all five
/// views' `section_active` shadows**. `SectionActiveGate` fires on transitions only and
/// its `ChangeTracker` baselines silently inside `AppWindow::new()`, so a shadow seeded
/// against the nav index alone answers `true` for every tab and the four that aren't
/// mounted never get an edge to correct them.
pub fn tab_is_mounted(ui: &AppWindow, tab: MyLibraryTab) -> bool {
    if ui.global::<Nav>().get_selected_index() != super::NAV_MY_LIBRARY {
        return false;
    }
    let g = ui.global::<MyLibrary>();
    tab_from_index(&g, g.get_tab_idx()) == tab
}

/// **The window inside which a hero global is still reachable without a fetch.** Every tab
/// is one pick away and a tab leave clears no detail id, so a detail left behind on
/// another tab morphs its banner back open the moment that tab is picked, ahead of the
/// re-fetch. A teardown inside this window hands nothing back; what does is
/// `my_library/callbacks.rs`'s pair — `hero-collapsed` for a genuine close, the page's own
/// teardown for the leave.
///
/// Deliberately *not* the section gate's predicate, which also goes false when Now Playing
/// or the miniplayer covers the band. Covering it is not leaving it.
pub fn the_band_is_up(ui: &AppWindow) -> bool {
    ui.global::<Nav>().get_selected_index() == super::NAV_MY_LIBRARY
}

/// The detail `tab` has open, or `None` — including for Songs, which has no detail.
///
/// **The tab is what discriminates, not the id.** `seed_detail_from_settings` runs for all
/// four detail views at boot whichever tab is restored, so more than one `*Detail.*-id`
/// can be `>= 0` and "some id is set" answers nothing.
///
/// Takes the tab rather than reading the mounted one because `nav_history` genuinely asks
/// about an unmounted one, resolving a *recorded* entry's detail.
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

/// Which of the page's nine surfaces is on screen — **one answer to a question the page
/// asks four ways**: routing a keystroke, reading a filter back, rewinding a count,
/// resolving a history entry, all off the same *(tab, is its detail open)* pair. Callers
/// keep only what is genuinely per-surface: which global to write, which callback it
/// fires.
///
/// Two askers stay off it, both answering a *different* question about the same globals:
/// `hero_chips::my_library_owner` wants the `ChipOwner` a published row was stamped with,
/// not always the mounted tab's, and `tasks::rss_sampler::my_library_tag` a diagnostic
/// string.
pub fn mounted_surface(ui: &AppWindow) -> MountedSurface {
    let tab = {
        let g = ui.global::<MyLibrary>();
        tab_from_index(&g, g.get_tab_idx())
    };
    MountedSurface {
        tab,
        detail_id: detail_id_for(ui, tab),
    }
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

/// Close whatever detail `tab` has open. The band's back arrow and a Mouse-4 step out are
/// the same act, so they share the dispatch rather than each spelling the five arms; what
/// each `close-detail` handler then does stays where it is.
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
/// shadow at wire time from the nav index *and* this tab. Seed afterwards and every one
/// answers for the global's declared `0` — Songs active, the persisted tab inactive — with
/// `SectionActiveGate`'s `ChangeTracker` already baselined and no edge left to correct it.
///
/// Stateless, unlike the two curated pages' `seed_tab`: there is no handle to seed.
pub fn seed_tab(ui: &AppWindow, persisted_tab: i32) {
    let g = ui.global::<MyLibrary>();
    g.set_tab_idx(crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count()));
}

/// Land on a My Library tab from outside the tab bar — the destination half of a
/// cross-section drill, which moves the nav index as well as the tab. [`persist_tab`]
/// rather than `tab-changed`, a drill not being a pick; the tab goes first, so the page
/// mounts on the body it is meant to show.
pub fn go_to_tab(ui: &AppWindow, tab: i32) {
    persist_tab(ui, tab);
    let nav = ui.global::<Nav>();
    nav.set_selected_index(super::NAV_MY_LIBRARY);
    nav.invoke_persist_selected_index(super::NAV_MY_LIBRARY);
}

/// Return to the section a drill started from, recorded as `origin-nav-index`. Called from
/// each detail's `on_close_detail` **before** it clears its own id, so Slint reroutes in
/// one frame rather than flashing the grid underneath.
///
/// **Only a drill from another section records one.** The band's back arrow means "close
/// this detail" and the tab bar names the detail's own tab for the whole visit, so a drill
/// between two tabs of this page restores nothing. Mouse-4/5 walks the real history.
pub fn return_to_section(ui: &AppWindow, origin_nav: i32) {
    let nav = ui.global::<Nav>();
    nav.set_selected_index(origin_nav);
    nav.invoke_persist_selected_index(origin_nav);
}
