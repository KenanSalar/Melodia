//! `Player.toggle-favorite` and `Player.set-current-rating` fan-out into
//! Tracks / Browse / `AlbumDetail` / `ArtistDetail` / `GenreDetail` rows.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::tracks::{self as tracks_ui_mod, TracksUi};
use crate::{AppWindow, Player};

/// Wire `Player.toggle-favorite` (heart in the Now Playing view, the
/// now-playing-bar, and the queue/track overflow menu). Mirrors the
/// resulting `(id, fav)` into every view-side surface that holds a
/// per-row `is_favorite`, so a row showing the currently-playing track
/// updates instantly regardless of which view it sits in. Each
/// `apply_*` helper walks its `VecModel` and silently no-ops when the
/// id isn't present, so calling all five on every toggle is safe.
///
/// Call once after `wire_tracks`, `wire_browse`, `wire_albums`,
/// `wire_artists`, and `wire_genres`.
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
    let tu = tracks_ui.clone();
    let bu = browse_ui.clone();
    let au = albums_ui.clone();
    let aru = artists_ui.clone();
    let gu = genres_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Player>().on_toggle_favorite(move || {
        let s = s.clone();
        let tu = tu.clone();
        let bu = bu.clone();
        let au = au.clone();
        let aru = aru.clone();
        let gu = gu.clone();
        let weak = weak.clone();
        s.runtime.clone().spawn(async move {
            match library::favorites::toggle_current_favorite(&s).await {
                Ok(Some((id, fav))) => {
                    tu.flip_favorite(id, fav);
                    tracks_ui_mod::apply_row_favorite(&weak, id, fav);
                    bu.flip_favorite(id, fav);
                    browse_ui_mod::apply_row_favorite(&weak, id, fav);
                    au.flip_detail_favorite(id, fav);
                    albums_ui_mod::apply_detail_row_favorite(&weak, id, fav);
                    aru.flip_detail_favorite(id, fav);
                    artists_ui_mod::apply_detail_row_favorite(&weak, id, fav);
                    gu.flip_detail_favorite(id, fav);
                    genres_ui_mod::apply_detail_row_favorite(&weak, id, fav);
                }
                Ok(None) => {}
                Err(e) => log::warn!("toggle_favorite: {e}"),
            }
        });
    });
}

/// Wire `Player.set-current-rating` (star control in the Now Playing view and
/// the overflow menu). The star-rating analogue of [`wire_now_playing_favorite`]:
/// rates the currently-playing track and mirrors the `(id, rating)` result into
/// every view-side surface that holds a per-row `rating`, so a row showing the
/// currently-playing track updates instantly. Each `apply_*` helper no-ops when
/// the id isn't present, so calling all five on every change is safe.
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
    let tu = tracks_ui.clone();
    let bu = browse_ui.clone();
    let au = albums_ui.clone();
    let aru = artists_ui.clone();
    let gu = genres_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Player>().on_set_current_rating(move |rating| {
        let s = s.clone();
        let tu = tu.clone();
        let bu = bu.clone();
        let au = au.clone();
        let aru = aru.clone();
        let gu = gu.clone();
        let weak = weak.clone();
        s.runtime.clone().spawn(async move {
            match library::ratings::set_current_rating(&s, rating).await {
                Ok(Some((id, rating))) => {
                    tu.flip_rating(id, rating);
                    tracks_ui_mod::apply_row_rating(&weak, id, rating);
                    bu.flip_rating(id, rating);
                    browse_ui_mod::apply_row_rating(&weak, id, rating);
                    au.flip_detail_rating(id, rating);
                    albums_ui_mod::apply_detail_row_rating(&weak, id, rating);
                    aru.flip_detail_rating(id, rating);
                    artists_ui_mod::apply_detail_row_rating(&weak, id, rating);
                    gu.flip_detail_rating(id, rating);
                    genres_ui_mod::apply_detail_row_rating(&weak, id, rating);
                }
                Ok(None) => {}
                Err(e) => log::warn!("set_current_rating: {e}"),
            }
        });
    });
}
