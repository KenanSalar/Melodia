//! `Nav.go-to-album` / `go-to-artist` / `go-to-genre`, which every right-click menu's "Go to …"
//! calls straight. Wired after every `wire_*`, so all three target handles exist.
//!
//! A drill stamps `*Detail.origin-nav-index` synchronously from where the user stands
//! ([`origin_stamp`]), marks the slide, spawns `open_*_with` and moves the nav index and tab in
//! its completion closure, then persists `last_detail_ids[target_view]` so a restart reopens the
//! same detail.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::my_library::{NAV_MY_LIBRARY, NO_TAB, go_to_tab, tab_of_section};
use crate::ui::nav_transition;
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{
    AlbumDetail, AppWindow, ArtistDetail, GenreDetail, MyLibrary, Nav, NavEnterFrom, Queue,
};

/// Where the user stands when a "Go to …" fires: the nav index, plus the My Library tab when
/// that index *is* My Library. Read synchronously, before any await.
#[derive(Clone, Copy)]
pub(in crate::ui) struct Origin {
    nav: i32,
    tab: i32,
}

impl Origin {
    /// A section that has no tabs — Favorites, Search and the rest.
    pub(in crate::ui) fn section(nav: i32) -> Self {
        Self { nav, tab: NO_TAB }
    }

    /// Read the current position off the globals. UI thread only.
    pub(in crate::ui) fn read(ui: &AppWindow) -> Self {
        let nav = ui.global::<Nav>().get_selected_index();
        Self { nav, tab: tab_of_section(ui, nav) }
    }

    /// What the destination stamps as its `origin-nav-index`. See [`origin_stamp`].
    fn stamp(self) -> i32 {
        origin_stamp(self.nav)
    }

    /// Whether the user is still where the drill started, so the fetch's completion doesn't
    /// move them after they navigated away. The tab half is what tells within My Library.
    fn still_current(self, ui: &AppWindow) -> bool {
        ui.global::<Nav>().get_selected_index() == self.nav
            && (self.nav != NAV_MY_LIBRARY || ui.global::<MyLibrary>().get_tab_idx() == self.tab)
    }
}

/// The `origin-nav-index` a drill starting at section `nav` records: `-1` when that
/// section *is* My Library, the index itself otherwise.
///
/// **An origin means "another section".** The four destinations are all tabs of one page, so a
/// drill that starts there ends there: the tab bar names the detail's own tab for the whole
/// visit, and a back arrow restoring the tab it came from would contradict it. The arrow means
/// "close this detail"; Mouse-4/5 walks the real history. A free fn so it tests without an
/// `AppWindow`.
pub(in crate::ui) fn origin_stamp(nav: i32) -> i32 {
    if nav == NAV_MY_LIBRARY { -1 } else { nav }
}

/// Wire the three `Nav.go-to-*` callbacks to their cross-tab handlers. Must run *after* the
/// three target `install`s, so their UI handles exist.
pub fn wire_cross_tab_nav(
    ui: &AppWindow,
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
    genres_ui: &Arc<GenresUi>,
) {
    let weak = ui.as_weak();
    let nav = ui.global::<Nav>();
    nav.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    nav.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    nav.on_go_to_genre(make_go_to_genre(state, genres_ui, weak));
}

/// Moves to the drill's tab once it has landed, uncovering the content panel first: the bar
/// stays reachable over Now Playing and the floating queue, and a detail opened under either is
/// one nobody sees. The docked panel stays, beside the page rather than over it.
fn land(ui: &AppWindow, tab: i32) {
    ui.global::<Nav>().set_now_playing_open(false);
    let queue = ui.global::<Queue>();
    if queue.get_floating() {
        queue.invoke_close();
    }
    go_to_tab(ui, tab);
}

fn make_go_to_album(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
) -> impl Fn(i32) + 'static {
    let s = state.clone();
    let au = albums_ui.clone();
    move |album_id| {
        let id = i64::from(album_id);
        let Some(ui) = weak.upgrade() else { return };
        let origin = Origin::read(&ui);
        nav_transition::mark(&ui, NavEnterFrom::Right);
        open_album_cross_tab(&s, &au, &weak, id, origin, "cross_tab_nav::go_to_album");
    }
}

fn make_go_to_artist(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
) -> impl Fn(i32) + 'static {
    let s = state.clone();
    let au = artists_ui.clone();
    move |artist_id| {
        let id = i64::from(artist_id);
        let Some(ui) = weak.upgrade() else { return };
        let origin = Origin::read(&ui);
        nav_transition::mark(&ui, NavEnterFrom::Right);
        open_artist_cross_tab(&s, &au, &weak, id, origin, "cross_tab_nav::go_to_artist");
    }
}

/// The shared "open Album Detail under the Albums tab" hand-off, for every "Go to Album" and the
/// Search result's album card.
///
/// Sets no transition direction, `open_album_with` marking it from its argument; a caller that
/// wants an early `nav_transition::mark` for the same-tab case calls it first.
pub(in crate::ui) fn open_album_cross_tab(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: &Weak<AppWindow>,
    album_id: i64,
    origin: Origin,
    log_tag: &'static str,
) {
    let Some(ui) = weak.upgrade() else { return };
    ui.global::<AlbumDetail>().set_origin_nav_index(origin.stamp());
    let target_tab = ui.global::<MyLibrary>().get_tab_albums();

    let s_fetch = state.clone();
    let au_fetch = albums_ui.clone();
    let weak_fetch = weak.clone();
    state.runtime.clone().spawn(async move {
        if let Err(e) = albums_ui_mod::open_album_with(
            &s_fetch,
            &au_fetch,
            weak_fetch,
            album_id,
            NavEnterFrom::Right,
            move |ui: &AppWindow| {
                if origin.still_current(ui) {
                    land(ui, target_tab);
                }
            },
        )
        .await
        {
            log::warn!("{log_tag}({album_id}): {}", describe(&e));
        }
    });

    let s_disk = state.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_last_detail_id(
            &s_disk.paths,
            view_id::ALBUM_DETAIL,
            Some(album_id),
        ) {
            log::warn!("{log_tag} persist: {}", describe(&e));
        }
    });
}

/// Artist counterpart of [`open_album_cross_tab`], moving to the Artists tab.
pub(in crate::ui) fn open_artist_cross_tab(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: &Weak<AppWindow>,
    artist_id: i64,
    origin: Origin,
    log_tag: &'static str,
) {
    let Some(ui) = weak.upgrade() else { return };
    ui.global::<ArtistDetail>().set_origin_nav_index(origin.stamp());
    let target_tab = ui.global::<MyLibrary>().get_tab_artists();

    let s_fetch = state.clone();
    let au_fetch = artists_ui.clone();
    let weak_fetch = weak.clone();
    state.runtime.clone().spawn(async move {
        if let Err(e) = artists_ui_mod::open_artist_with(
            &s_fetch,
            &au_fetch,
            weak_fetch,
            artist_id,
            NavEnterFrom::Right,
            move |ui: &AppWindow| {
                if origin.still_current(ui) {
                    land(ui, target_tab);
                }
            },
        )
        .await
        {
            log::warn!("{log_tag}({artist_id}): {}", describe(&e));
        }
    });

    let s_disk = state.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_last_detail_id(
            &s_disk.paths,
            view_id::ARTIST_DETAIL,
            Some(artist_id),
        ) {
            log::warn!("{log_tag} persist: {}", describe(&e));
        }
    });
}

fn make_go_to_genre(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
) -> impl Fn(i32) + 'static {
    let s = state.clone();
    let gu = genres_ui.clone();
    move |genre_id| {
        let id = i64::from(genre_id);
        let Some(ui) = weak.upgrade() else { return };

        let origin = Origin::read(&ui);
        ui.global::<GenreDetail>().set_origin_nav_index(origin.stamp());
        let target_tab = ui.global::<MyLibrary>().get_tab_genres();
        nav_transition::mark(&ui, NavEnterFrom::Right);

        let s_fetch = s.clone();
        let gu_fetch = gu.clone();
        let weak_fetch = weak.clone();
        s.runtime.clone().spawn(async move {
            if let Err(e) = genres_ui_mod::open_genre_with(
                &s_fetch,
                &gu_fetch,
                weak_fetch,
                id,
                NavEnterFrom::Right,
                move |ui: &AppWindow| {
                    if origin.still_current(ui) {
                        land(ui, target_tab);
                    }
                },
            )
            .await
            {
                log::warn!("cross_tab_nav::go_to_genre({id}): {}", describe(&e));
            }
        });

        let s_disk = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::settings::set_last_detail_id(
                &s_disk.paths,
                view_id::GENRE_DETAIL,
                Some(id),
            ) {
                log::warn!("cross_tab_nav::go_to_genre persist: {}", describe(&e));
            }
        });
    }
}

#[cfg(test)]
#[path = "tests/cross_tab_nav_tests.rs"]
mod tests;
