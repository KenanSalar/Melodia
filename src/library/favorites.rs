use std::sync::Arc;

use crate::database::queries;
use crate::entities::{album, artist, track};
use crate::error::AppError;
use crate::player::state::{PlayerAction, lock_state, with_state_emit};
use crate::state::AppState;

pub async fn set_favorite(
    state: &AppState,
    ids: Vec<i64>,
    favorite: bool,
) -> Result<(), AppError> {
    queries::track::set_favorite(&state.db, &ids, favorite).await?;
    state
        .library_changed_tx
        .send_modify(|n| *n = n.wrapping_add(1));
    Ok(())
}

/// Toggle the favorite flag on the currently playing track. Persists to DB,
/// flips `is_favorite` on `PlayerState.current_track` (so the next emit
/// rebuilds the view-model with the new value), and returns the affected
/// `(id, new_fav)` so callers can mirror the change into other UI surfaces
/// (e.g. the tracks list) without re-locking.
pub async fn toggle_current_favorite(
    state: &AppState,
) -> Result<Option<(i64, bool)>, AppError> {
    let Some((id, new_fav)) = ({
        let g = lock_state(&state.player_state);
        g.current_track.as_ref().map(|t| (t.id, !t.is_favorite))
    }) else {
        return Ok(None);
    };

    queries::track::set_favorite(&state.db, &[id], new_fav).await?;

    with_state_emit(&state.player_state, &state.sinks, |s| {
        if let Some(track) = s.current_track.as_mut() {
            Arc::make_mut(track).is_favorite = new_fav;
        }
        Vec::<PlayerAction>::new()
    });

    state
        .library_changed_tx
        .send_modify(|n| *n = n.wrapping_add(1));

    Ok(Some((id, new_fav)))
}

pub async fn get_favorite_tracks(
    state: &AppState,
    sort_by: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_favorite_tracks_for_list(&state.db, sort_by, sort_dir).await
}

pub async fn get_favorite_stats(state: &AppState) -> Result<track::FavoriteStats, AppError> {
    queries::track::get_favorite_stats(&state.db).await
}

pub async fn get_favorite_albums(
    state: &AppState,
) -> Result<Vec<album::FavoriteAlbum>, AppError> {
    queries::album::get_favorite_albums(&state.db).await
}

pub async fn get_favorite_artists(
    state: &AppState,
) -> Result<Vec<artist::FavoriteArtist>, AppError> {
    queries::artist::get_favorite_artists(&state.db).await
}

pub async fn get_most_played_favorites(
    state: &AppState,
    limit: i64,
) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    let limit = limit.clamp(1, 100);
    queries::track::get_most_played_favorites(&state.db, limit).await
}
