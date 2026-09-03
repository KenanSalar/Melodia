//! `Player.toggle-favorite` and `Player.set-current-rating` fan-out into
//! Tracks / Browse / `AlbumDetail` / `ArtistDetail` / `GenreDetail` rows.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::tracks::{self as tracks_ui_mod, TracksUi};
use crate::{AppWindow, Player};
use melodia_app::library;
use melodia_app::state::AppState;

/// The five view-side surfaces holding a per-row `is_favorite` / `rating`,
/// bundled so the two Now-Playing fan-outs clone one handle instead of six.
/// Each `apply_*` helper walks its `VecModel` and silently no-ops when the
/// id isn't present, so mirroring into all five on every change is safe.
#[derive(Clone)]
struct CurrentTrackMirrors {
    tracks: Arc<TracksUi>,
    browse: Arc<BrowseUi>,
    albums: Arc<AlbumsUi>,
    artists: Arc<ArtistsUi>,
    genres: Arc<GenresUi>,
    weak: slint::Weak<AppWindow>,
}

impl CurrentTrackMirrors {
    fn mirror_favorite(&self, id: i64, fav: bool) {
        self.tracks.flip_favorite(id, fav);
        tracks_ui_mod::apply_row_favorite(&self.weak, id, fav);
        self.browse.flip_favorite(id, fav);
        browse_ui_mod::apply_row_favorite(&self.weak, id, fav);
        self.albums.flip_detail_favorite(id, fav);
        albums_ui_mod::apply_detail_row_favorite(&self.weak, id, fav);
        self.artists.flip_detail_favorite(id, fav);
        artists_ui_mod::apply_detail_row_favorite(&self.weak, id, fav);
        self.genres.flip_detail_favorite(id, fav);
        genres_ui_mod::apply_detail_row_favorite(&self.weak, id, fav);
    }

    fn mirror_rating(&self, id: i64, rating: i32) {
        self.tracks.flip_rating(id, rating);
        tracks_ui_mod::apply_row_rating(&self.weak, id, rating);
        self.browse.flip_rating(id, rating);
        browse_ui_mod::apply_row_rating(&self.weak, id, rating);
        self.albums.flip_detail_rating(id, rating);
        albums_ui_mod::apply_detail_row_rating(&self.weak, id, rating);
        self.artists.flip_detail_rating(id, rating);
        artists_ui_mod::apply_detail_row_rating(&self.weak, id, rating);
        self.genres.flip_detail_rating(id, rating);
        genres_ui_mod::apply_detail_row_rating(&self.weak, id, rating);
    }
}

fn mirrors(
    ui: &AppWindow,
    tracks_ui: &Arc<TracksUi>,
    browse_ui: &Arc<BrowseUi>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
    genres_ui: &Arc<GenresUi>,
) -> CurrentTrackMirrors {
    CurrentTrackMirrors {
        tracks: tracks_ui.clone(),
        browse: browse_ui.clone(),
        albums: albums_ui.clone(),
        artists: artists_ui.clone(),
        genres: genres_ui.clone(),
        weak: ui.as_weak(),
    }
}

/// Wire `Player.toggle-favorite` (heart in the Now Playing view, the
/// now-playing-bar, and the queue/track overflow menu). Mirrors the
/// resulting `(id, fav)` into every view-side surface that holds a
/// per-row `is_favorite`, so a row showing the currently-playing track
/// updates instantly regardless of which view it sits in.
///
/// Call once after `tracks::install`, `browse::install`, `albums::install`,
/// `artists::install` and `genres::install`.
pub fn wire_now_playing_favorite(
    ui: &AppWindow,
    state: &AppState,
    tracks_ui: &Arc<TracksUi>,
    browse_ui: &Arc<BrowseUi>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
    genres_ui: &Arc<GenresUi>,
) {
    let s = state.clone();
    let m = mirrors(ui, tracks_ui, browse_ui, albums_ui, artists_ui, genres_ui);
    ui.global::<Player>().on_toggle_favorite(move || {
        let s = s.clone();
        let m = m.clone();
        s.runtime.clone().spawn(async move {
            match library::favorites::toggle_current_favorite(&s).await {
                Ok(Some((id, fav))) => m.mirror_favorite(id, fav),
                Ok(None) => {}
                Err(e) => log::warn!("toggle_favorite: {e}"),
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
pub fn wire_now_playing_rating(
    ui: &AppWindow,
    state: &AppState,
    tracks_ui: &Arc<TracksUi>,
    browse_ui: &Arc<BrowseUi>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
    genres_ui: &Arc<GenresUi>,
) {
    let s = state.clone();
    let m = mirrors(ui, tracks_ui, browse_ui, albums_ui, artists_ui, genres_ui);
    ui.global::<Player>().on_set_current_rating(move |rating| {
        let s = s.clone();
        let m = m.clone();
        s.runtime.clone().spawn(async move {
            match library::ratings::set_current_rating(&s, rating).await {
                Ok(Some((id, rating))) => m.mirror_rating(id, rating),
                Ok(None) => {}
                Err(e) => log::warn!("set_current_rating: {e}"),
            }
        });
    });
}
