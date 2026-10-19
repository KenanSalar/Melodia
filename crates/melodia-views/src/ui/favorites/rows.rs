//! The Slint models the view writes into, and the row mapper Favorite Artists fills its cards with.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::grids::{FavoriteArtistsGrid, FavoriteMostPlayed, MOST_PLAYED_LABEL};
use crate::ui::card_grid;
use crate::ui::paged_rows::ScrollPoint;
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::util::{clamp_i64_to_i32, opt_shared};
use melodia_app::state::AppState;
use melodia_core::entities::artist::FavoriteArtist;
use melodia_ui::{AppWindow, EntityStripRow as UiEntityStripRow, Favorites};

/// Where each of the page's three lists was last read, for the loads that seed them.
#[derive(Default)]
pub(super) struct Scrolls {
    pub songs: ScrollPoint,
    pub most_played: ScrollPoint,
    pub artists: ScrollPoint,
}

/// Bind the page's models, each installed once: the Songs list a [`TrackRows`], the two grid tabs
/// a `PagedGrid` each, and the selection set a `VecModel`. Later updates locate them by
/// downcasting back from the UI thread.
pub(super) fn install_favorites_models(ui: &AppWindow, state: &AppState) -> Scrolls {
    let g = ui.global::<Favorites>();

    let most_played = card_grid::install::<FavoriteMostPlayed>(state, ui);
    let most_played_scroll = most_played.scroll_point();
    g.set_most_played_rows(ModelRc::from(most_played));

    let artists = card_grid::install::<FavoriteArtistsGrid>(state, ui);
    let artists_scroll = artists.scroll_point();
    g.set_artist_rows(ModelRc::from(artists));

    let tracks: Rc<TrackRows> = TrackRows::new(track_rows::fetcher(state));
    let songs = tracks.scroll_point();
    g.set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
    Scrolls { songs, most_played: most_played_scroll, artists: artists_scroll }
}

/// The Most Played grid's track ids, in card order.
pub fn most_played_ids(ui: &AppWindow) -> Vec<i64> {
    crate::ui::most_played::shown_ids(
        &ui.global::<Favorites>().get_most_played_rows(),
        MOST_PLAYED_LABEL,
    )
}

/// Map a `FavoriteArtist` + caller-supplied subtitle to its Slint card
/// row. The subtitle is the translated `"{n} favorite[s]"` count line and
/// must be resolved on the UI thread via `Favorites.artist-favorite-subtitle(count)`
/// (Slint 1.18 doesn't expose `translate_from_bundle` to Rust). `play_count`
/// is unused.
pub fn to_slint_fav_artist_row(a: &FavoriteArtist, subtitle: SharedString) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle,
        artwork_path: opt_shared(a.image_path.as_deref()),
        play_count: 0,
        is_favorite: false,
    }
}
