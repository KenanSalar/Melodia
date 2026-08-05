//! Single-home wiring for the `go-to-album` / `go-to-artist` /
//! `go-to-genre` callbacks every track-list view's right-click menu
//! emits. Lives in its own module because each per-view callback file
//! would otherwise need access to all three target UI handles
//! (`AlbumsUi`, `ArtistsUi`, `GenresUi`); instead, this runs after every
//! per-view `wire_*` so the handles are guaranteed to exist.
//!
//! Behaviour mirrors the existing cross-tab open-album/artist hand-offs
//! (`callbacks/artists.rs::on_open_album`, `callbacks/favorites.rs::
//! on_open_artist`):
//!
//! 1. Stamp `*Detail.origin-nav-index` + `origin-tab` from where the user is
//!    standing, synchronously, so the back arrow returns there.
//! 2. Mark `Nav.pending-enter-from = Right` (drill-in slide).
//! 3. Spawn `open_*` / `open_*_with`; move `Nav.selected-index` and
//!    `MyLibrary.tab-idx` to the target inside the `upgrade_in_event_loop`
//!    closure.
//! 4. Persist `views.json`'s `last_detail_ids[target_view]` so a restart
//!    reopens the same detail.
//!
//! **An origin is a pair, not an index.** The four destinations are all tabs of My
//! Library now, so a drill starting *inside* that page stamps `origin-nav-index = 3`
//! and then navigates to 3 — which makes the "did the user navigate away mid-fetch"
//! guard trivially true and the back path's `selected-index` restore a no-op that
//! leaves the wrong tab mounted. `origin-tab` is the discriminator, and it lives per
//! detail global because two details can be open at once.
//!
//! Search-view exception: the Slint side rewires its row context menu
//! to call the existing `Search.open-album` / `Search.open-artist`
//! (already wired in `callbacks/search.rs`). Only `go-to-genre` is new
//! for Search — wired below alongside the other six globals.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::library;
use crate::state::AppState;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::my_library::NAV_MY_LIBRARY;
use crate::ui::nav_transition;
use crate::ui::track_list_view::view_id;
use crate::{
    AlbumDetail, AppWindow, ArtistDetail, Browse, Favorites, GenreDetail, MyLibrary, Nav,
    NavEnterFrom, PlaylistDetail, RecentlyPlayed, Search, Tracks,
};

/// Where the user is standing when a "Go to …" fires: the nav index, plus the My
/// Library tab when that index *is* My Library ([`NO_TAB`] otherwise). Both halves are
/// read synchronously, before any await, and stamped onto the destination's detail
/// global so its back arrow can restore the pair.
#[derive(Clone, Copy)]
pub(super) struct Origin {
    nav: i32,
    tab: i32,
}

/// [`Origin::tab`] for an origin outside My Library.
const NO_TAB: i32 = -1;

impl Origin {
    /// A section that has no tabs — Favorites, Search and the rest.
    pub(super) fn section(nav: i32) -> Self {
        Self { nav, tab: NO_TAB }
    }

    /// Read the current position off the globals. UI thread only.
    pub(super) fn read(ui: &AppWindow) -> Self {
        let nav = ui.global::<Nav>().get_selected_index();
        let tab = if nav == NAV_MY_LIBRARY {
            ui.global::<MyLibrary>().get_tab_idx()
        } else {
            NO_TAB
        };
        Self { nav, tab }
    }

    /// Whether the user is still where the drill started. Guards the destination flip
    /// inside the fetch's completion closure — moving them after they navigated away
    /// mid-fetch is exactly what this exists to prevent, and the nav index alone stopped
    /// being able to tell once the four destinations became tabs of one page.
    fn still_current(self, ui: &AppWindow) -> bool {
        ui.global::<Nav>().get_selected_index() == self.nav
            && (self.nav != NAV_MY_LIBRARY
                || ui.global::<MyLibrary>().get_tab_idx() == self.tab)
    }
}

/// Move to a My Library tab and persist both halves of the destination, the way a
/// sidebar click and a tab pick each persist their own. The tab is written **before**
/// the nav index so the page mounts on the body it is meant to show.
fn go_to_tab(ui: &AppWindow, tab: i32) {
    let g = ui.global::<MyLibrary>();
    g.set_tab_idx(tab);
    g.invoke_persist_tab_idx(tab);
    let nav = ui.global::<Nav>();
    nav.set_selected_index(NAV_MY_LIBRARY);
    nav.invoke_persist_selected_index(NAV_MY_LIBRARY);
}

/// Wire every `*.go-to-album` / `*.go-to-artist` / `*.go-to-genre`
/// callback to its cross-tab navigation handler. Must be called *after*
/// `wire_albums` / `wire_artists` / `wire_genres` so the per-target UI
/// handles exist.
pub fn wire_cross_tab_nav(
    ui: &AppWindow,
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
    genres_ui: &Arc<GenresUi>,
) {
    let weak = ui.as_weak();

    // --- Tracks ------------------------------------------------------
    let g = ui.global::<Tracks>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- Browse ------------------------------------------------------
    let g = ui.global::<Browse>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- Favorites ---------------------------------------------------
    let g = ui.global::<Favorites>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- PlaylistDetail ----------------------------------------------
    let g = ui.global::<PlaylistDetail>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- AlbumDetail (cross-tab from Album Detail's track list) ------
    // The row menu hides "Go to Album" inside Album Detail, but
    // "Go to Artist" / "Go to Genre" stay live.
    let g = ui.global::<AlbumDetail>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- ArtistDetail ------------------------------------------------
    let g = ui.global::<ArtistDetail>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- GenreDetail -------------------------------------------------
    let g = ui.global::<GenreDetail>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- RecentlyPlayed ----------------------------------------------
    let g = ui.global::<RecentlyPlayed>();
    g.on_go_to_album(make_go_to_album(state, albums_ui, weak.clone()));
    g.on_go_to_artist(make_go_to_artist(state, artists_ui, weak.clone()));
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak.clone()));

    // --- Search (only Go-to-Genre is new; Album / Artist reuse the
    // pre-existing `Search.open-album` / `Search.open-artist` already
    // wired in `callbacks/search.rs`) -------------------------------
    let g = ui.global::<Search>();
    g.on_go_to_genre(make_go_to_genre(state, genres_ui, weak));
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

/// Shared cross-tab "open the Album Detail under the Albums tab" hand-off,
/// used by every track-list "Go to Album" menu item and the Search-result
/// album card:
///
/// 1. Stamp `AlbumDetail.origin-nav-index` + `origin-tab` from `origin` so the back
///    arrow returns to where the user came from.
/// 2. Spawn `open_album_with`, moving to the Albums tab inside its
///    `upgrade_in_event_loop` closure (guarded so a nav or tab switch mid-fetch
///    doesn't yank the user away).
/// 3. Persist `views.json`'s `last_detail_ids[ALBUM_DETAIL]` so a restart
///    reopens the same detail.
///
/// The view-transition direction is intentionally **not** set here —
/// `open_album_with` marks it from its `NavEnterFrom::Right` argument.
/// Callers that also want an early `nav_transition::mark` (the track-list
/// menus do, to cover the same-tab case) must call it themselves before
/// invoking this helper.
pub(super) fn open_album_cross_tab(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: &Weak<AppWindow>,
    album_id: i64,
    origin: Origin,
    log_tag: &'static str,
) {
    let Some(ui) = weak.upgrade() else { return };
    let detail = ui.global::<AlbumDetail>();
    detail.set_origin_nav_index(origin.nav);
    detail.set_origin_tab(origin.tab);
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
                    go_to_tab(ui, target_tab);
                }
            },
        )
        .await
        {
            log::warn!("{log_tag}({album_id}): {e}");
        }
    });

    let s_disk = state.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) =
            library::settings::set_last_detail_id(&s_disk, view_id::ALBUM_DETAIL, Some(album_id))
        {
            log::warn!("{log_tag} persist: {e}");
        }
    });
}

/// Artist counterpart of [`open_album_cross_tab`] — see that function's
/// docs. Moves to the Artists tab and persists
/// `views.json`'s `last_detail_ids[ARTIST_DETAIL]`.
pub(super) fn open_artist_cross_tab(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: &Weak<AppWindow>,
    artist_id: i64,
    origin: Origin,
    log_tag: &'static str,
) {
    let Some(ui) = weak.upgrade() else { return };
    let detail = ui.global::<ArtistDetail>();
    detail.set_origin_nav_index(origin.nav);
    detail.set_origin_tab(origin.tab);
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
                    go_to_tab(ui, target_tab);
                }
            },
        )
        .await
        {
            log::warn!("{log_tag}({artist_id}): {e}");
        }
    });

    let s_disk = state.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) =
            library::settings::set_last_detail_id(&s_disk, view_id::ARTIST_DETAIL, Some(artist_id))
        {
            log::warn!("{log_tag} persist: {e}");
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
        // Stamp the origin synchronously so `genres::on_close_detail`
        // can restore the originating view on back-press. Without this,
        // the back arrow resets to the Genres grid instead of the
        // user's previous view.
        let detail = ui.global::<GenreDetail>();
        detail.set_origin_nav_index(origin.nav);
        detail.set_origin_tab(origin.tab);
        let target_tab = ui.global::<MyLibrary>().get_tab_genres();
        nav_transition::mark(&ui, NavEnterFrom::Right);

        let s_fetch = s.clone();
        let gu_fetch = gu.clone();
        let weak_fetch = weak.clone();
        s.runtime.clone().spawn(async move {
            if let Err(e) = genres_ui_mod::open_genre(
                &s_fetch,
                &gu_fetch,
                weak_fetch.clone(),
                id,
                NavEnterFrom::Right,
            )
            .await
            {
                log::warn!("cross_tab_nav::go_to_genre({id}): {e}");
                return;
            }
            let _ = weak_fetch.upgrade_in_event_loop(move |ui| {
                if origin.still_current(&ui) {
                    go_to_tab(&ui, target_tab);
                }
            });
        });

        let s_disk = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) =
                library::settings::set_last_detail_id(&s_disk, view_id::GENRE_DETAIL, Some(id))
            {
                log::warn!("cross_tab_nav::go_to_genre persist: {e}");
            }
        });
    }
}
