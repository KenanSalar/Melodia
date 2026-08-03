//! The Slint models the view writes into, and the row mappers that fill them.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::artist::FavoriteArtist;
use crate::entities::track::MostPlayedFavorite;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AppWindow, EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow, Favorites,
    TrackListRow as UiTrackListRow,
};

/// Bind empty Slint `VecModel`s for the two grid tabs, the Songs list, the
/// selection set, and the mosaic-path string list. Subsequent updates locate
/// them by downcasting back to `VecModel<T>` from the UI thread.
pub fn install_favorites_models(ui: &AppWindow) {
    let g = ui.global::<Favorites>();

    let most_played: Rc<VecModel<UiEntityGridRow>> = Rc::new(VecModel::default());
    g.set_most_played_rows(ModelRc::from(most_played));

    let artists: Rc<VecModel<UiEntityGridRow>> = Rc::new(VecModel::default());
    g.set_artist_rows(ModelRc::from(artists));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    g.set_tracks(ModelRc::from(tracks));

    let mosaic_paths: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
    g.set_mosaic_paths(ModelRc::from(mosaic_paths));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
}

/// Map a `MostPlayedFavorite` to its Slint card row. Subtitle is the
/// artist name. `play_count` rides in the `play_count` slot so the grid's
/// `show-play-count: true` reveals the badge.
pub fn to_slint_most_played_row(t: &MostPlayedFavorite) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(t.id),
        title: SharedString::from(t.title.as_str()),
        subtitle: SharedString::from(t.artist.as_deref().unwrap_or("")),
        artwork_path: SharedString::from(t.artwork_path.as_deref().unwrap_or("")),
        play_count: t.play_count,
    }
}

/// Map a `FavoriteArtist` + caller-supplied subtitle to its Slint card
/// row. The subtitle is the translated "{n} favorite[s]" count line and
/// must be resolved on the UI thread via `Favorites.artist-favorite-subtitle(count)`
/// (Slint 1.16 doesn't expose `translate_from_bundle` to Rust). `play_count`
/// is unused.
pub fn to_slint_fav_artist_row(a: &FavoriteArtist, subtitle: SharedString) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle,
        artwork_path: SharedString::from(a.image_path.as_deref().unwrap_or("")),
        play_count: 0,
    }
}
