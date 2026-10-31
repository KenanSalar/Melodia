//! The fan-out of a heart or a star into every surface holding a per-row copy of it, for a write
//! made off any one list: the Now Playing heart and stars, and a removal from Favorites and its
//! undo.

use std::collections::HashSet;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::favorites::FavoritesUi;
use crate::ui::genres::GenresUi;
use crate::ui::paged_rows::track_rows;
use crate::ui::playlists::PlaylistsUi;
use crate::ui::queue_panel;
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::ui::search::{self as search_ui_mod, SearchUi};
use crate::ui::tracks::TracksUi;
use melodia_ui::{AppWindow, TrackListRow as UiTrackListRow};

/// Every surface holding a per-row heart or star. A patch no-ops where an id isn't listed, so
/// every change mirrors into all of them. Most reload on the `library_changed` bump behind the
/// write, so this moves their rows now rather than then; Search has no such reload, so its rows
/// move only through this.
#[derive(Clone)]
pub struct RowFlagMirrors {
    tracks: Arc<TracksUi>,
    browse: Arc<BrowseUi>,
    albums: Arc<AlbumsUi>,
    artists: Arc<ArtistsUi>,
    genres: Arc<GenresUi>,
    playlists: Arc<PlaylistsUi>,
    favorites: Arc<FavoritesUi>,
    recently_played: Arc<RecentlyPlayedUi>,
    search: Arc<SearchUi>,
    weak: slint::Weak<AppWindow>,
}

/// The handles [`RowFlagMirrors`] takes, bundled so the call site names each one.
#[derive(Clone, Copy)]
pub struct RowFlagHandles<'a> {
    pub tracks: &'a Arc<TracksUi>,
    pub browse: &'a Arc<BrowseUi>,
    pub albums: &'a Arc<AlbumsUi>,
    pub artists: &'a Arc<ArtistsUi>,
    pub genres: &'a Arc<GenresUi>,
    pub playlists: &'a Arc<PlaylistsUi>,
    pub favorites: &'a Arc<FavoritesUi>,
    pub recently_played: &'a Arc<RecentlyPlayedUi>,
    pub search: &'a Arc<SearchUi>,
}

impl RowFlagMirrors {
    /// Built once, after every `install` whose handle [`RowFlagHandles`] carries.
    pub fn new(ui: &AppWindow, handles: RowFlagHandles<'_>) -> Self {
        Self {
            tracks: handles.tracks.clone(),
            browse: handles.browse.clone(),
            albums: handles.albums.clone(),
            artists: handles.artists.clone(),
            genres: handles.genres.clone(),
            playlists: handles.playlists.clone(),
            favorites: handles.favorites.clone(),
            recently_played: handles.recently_played.clone(),
            search: handles.search.clone(),
            weak: ui.as_weak(),
        }
    }

    /// One hop per surface however many ids there are. Favorites drops an un-hearted row, its
    /// pick with it, and gets a re-hearted one back on its reload.
    pub(in crate::ui) fn favorites(&self, ids: &[i64], fav: bool) {
        self.patch_paged(ids, move |row| row.is_favorite = fav);
        if !fav {
            track_rows::remove_rows(&*self.favorites, ids.to_vec());
        }
        let set: HashSet<i64> = ids.iter().copied().collect();
        self.browse.flip_favorites(&set, fav);
        browse_ui_mod::apply_rows_favorite(&self.weak, set.clone(), fav);
        self.recently_played.flip_track_favorites(&set, fav);
        recently_played_ui_mod::apply_rows_favorite(&self.weak, set.clone(), fav);
        self.search.flip_favorites(&set, fav);
        search_ui_mod::apply_rows_favorite(&self.weak, set, fav);
        queue_panel::patch_rows(&self.weak, ids.to_vec(), move |row| row.is_favorite = fav);
    }

    /// [`Self::favorites`]' star-rating twin, for the one track Now Playing rates.
    pub(in crate::ui) fn rating(&self, id: i64, rating: i32) {
        let patch = move |row: &mut UiTrackListRow| row.rating = rating;
        self.patch_paged(&[id], patch);
        track_rows::patch_rows(&*self.favorites, vec![id], patch);
        let set = HashSet::from([id]);
        self.browse.flip_ratings(&set, rating);
        browse_ui_mod::apply_rows_rating(&self.weak, set.clone(), rating);
        self.recently_played.flip_track_ratings(&set, rating);
        recently_played_ui_mod::apply_rows_rating(&self.weak, set.clone(), rating);
        self.search.flip_ratings(&set, rating);
        search_ui_mod::apply_rows_rating(&self.weak, set, rating);
        queue_panel::patch_rows(&self.weak, vec![id], move |row| row.rating = rating);
    }

    /// Tracks and the four details, every `TrackRows` list but Favorites.
    fn patch_paged(&self, ids: &[i64], edit: impl Fn(&mut UiTrackListRow) + Copy + Send + 'static) {
        track_rows::patch_rows(&*self.tracks, ids.to_vec(), edit);
        track_rows::patch_rows(&*self.albums, ids.to_vec(), edit);
        track_rows::patch_rows(&*self.artists, ids.to_vec(), edit);
        track_rows::patch_rows(&*self.genres, ids.to_vec(), edit);
        track_rows::patch_rows(&*self.playlists, ids.to_vec(), edit);
    }
}
