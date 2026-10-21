//! `Player.toggle-favorite` and `Player.set-current-rating` fan-out into
//! Tracks / Browse / the four detail views / Search rows.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::genres::GenresUi;
use crate::ui::paged_rows::track_rows;
use crate::ui::playlists::PlaylistsUi;
use crate::ui::search::{self as search_ui_mod, SearchUi};
use crate::ui::tracks::TracksUi;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Player};

/// The view-side surfaces holding a per-row `is_favorite` / `rating`, bundled so the two
/// Now-Playing fan-outs clone one handle. Each patch no-ops where the id isn't listed, so
/// mirroring into all of them on every change is safe.
///
/// Search is here because the bar's heart is on screen while its results are, and it is the
/// one surface with no `library_changed` subscriber to correct a missed patch later.
#[derive(Clone)]
struct CurrentTrackMirrors {
    tracks: Arc<TracksUi>,
    browse: Arc<BrowseUi>,
    albums: Arc<AlbumsUi>,
    artists: Arc<ArtistsUi>,
    genres: Arc<GenresUi>,
    playlists: Arc<PlaylistsUi>,
    search: Arc<SearchUi>,
    weak: slint::Weak<AppWindow>,
}

impl CurrentTrackMirrors {
    fn mirror_favorite(&self, id: i64, fav: bool) {
        track_rows::patch_row(&*self.tracks, id, move |row| row.is_favorite = fav);
        self.browse.flip_favorite(id, fav);
        browse_ui_mod::apply_row_favorite(&self.weak, id, fav);
        track_rows::patch_row(&*self.albums, id, move |row| row.is_favorite = fav);
        track_rows::patch_row(&*self.artists, id, move |row| row.is_favorite = fav);
        track_rows::patch_row(&*self.genres, id, move |row| row.is_favorite = fav);
        track_rows::patch_row(&*self.playlists, id, move |row| row.is_favorite = fav);
        self.search.flip_favorite(id, fav);
        search_ui_mod::apply_row_favorite(&self.weak, id, fav);
    }

    fn mirror_rating(&self, id: i64, rating: i32) {
        track_rows::patch_row(&*self.tracks, id, move |row| row.rating = rating);
        self.browse.flip_rating(id, rating);
        browse_ui_mod::apply_row_rating(&self.weak, id, rating);
        track_rows::patch_row(&*self.albums, id, move |row| row.rating = rating);
        track_rows::patch_row(&*self.artists, id, move |row| row.rating = rating);
        track_rows::patch_row(&*self.genres, id, move |row| row.rating = rating);
        track_rows::patch_row(&*self.playlists, id, move |row| row.rating = rating);
        self.search.flip_rating(id, rating);
        search_ui_mod::apply_row_rating(&self.weak, id, rating);
    }
}

fn mirrors(ui: &AppWindow, handles: RowFlagHandles<'_>) -> CurrentTrackMirrors {
    CurrentTrackMirrors {
        tracks: handles.tracks.clone(),
        browse: handles.browse.clone(),
        albums: handles.albums.clone(),
        artists: handles.artists.clone(),
        genres: handles.genres.clone(),
        playlists: handles.playlists.clone(),
        search: handles.search.clone(),
        weak: ui.as_weak(),
    }
}

/// The handles both fan-outs take, borrowed rather than cloned — a parameter list this long
/// reads as an ordering nobody can check at the call site.
#[derive(Clone, Copy)]
pub struct RowFlagHandles<'a> {
    pub tracks: &'a Arc<TracksUi>,
    pub browse: &'a Arc<BrowseUi>,
    pub albums: &'a Arc<AlbumsUi>,
    pub artists: &'a Arc<ArtistsUi>,
    pub genres: &'a Arc<GenresUi>,
    pub playlists: &'a Arc<PlaylistsUi>,
    pub search: &'a Arc<SearchUi>,
}

/// Wire `Player.toggle-favorite` (heart in the Now Playing view, the
/// now-playing-bar, and the queue/track overflow menu). Mirrors the
/// resulting `(id, fav)` into every view-side surface that holds a
/// per-row `is_favorite`, so a row showing the currently-playing track
/// updates instantly regardless of which view it sits in.
///
/// Call once after every `install` whose handle [`RowFlagHandles`] carries.
pub fn wire_now_playing_favorite(ui: &AppWindow, state: &AppState, handles: RowFlagHandles<'_>) {
    let s = state.clone();
    let m = mirrors(ui, handles);
    ui.global::<Player>().on_toggle_favorite(move || {
        let s = s.clone();
        let m = m.clone();
        s.runtime.clone().spawn(async move {
            match library::favorites::toggle_current_favorite(&s).await {
                Ok(Some((id, fav))) => m.mirror_favorite(id, fav),
                Ok(None) => {}
                Err(e) => log::warn!("toggle_favorite: {}", describe(&e)),
            }
        });
    });
}

/// Wire `Player.set-current-rating` (star control in the Now Playing view and
/// the overflow menu). The star-rating analogue of [`wire_now_playing_favorite`]:
/// rates the currently-playing track and mirrors the `(id, rating)` result into
/// every view-side surface that holds a per-row `rating`.
///
/// Call once after the per-view wires, alongside `wire_now_playing_favorite`.
pub fn wire_now_playing_rating(ui: &AppWindow, state: &AppState, handles: RowFlagHandles<'_>) {
    let s = state.clone();
    let m = mirrors(ui, handles);
    ui.global::<Player>().on_set_current_rating(move |rating| {
        let s = s.clone();
        let m = m.clone();
        s.runtime.clone().spawn(async move {
            match library::ratings::set_current_rating(&s, rating).await {
                Ok(Some((id, rating))) => m.mirror_rating(id, rating),
                Ok(None) => {}
                Err(e) => log::warn!("set_current_rating: {}", describe(&e)),
            }
        });
    });
}
