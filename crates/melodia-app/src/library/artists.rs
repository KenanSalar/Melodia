//! Artist library API — thin wrappers over the `queries::artist` /
//! `queries::album` / `queries::track` layer, mirroring `library/albums.rs`.
//! Each function takes `&AppState` and returns `Result<_, AppError>`; the UI
//! bridge (`melodia-views`' `ui/artists/`) does the in-memory sorting / filtering /
//! chunking.

use crate::database::queries;
use crate::entities::{album, artist, track};
use crate::error::AppError;
use crate::state::AppState;

/// Every artist with at least one track, name-sorted (the `artist_stats`
/// view's default order). The Artist grid re-sorts in memory for the
/// track-count / album-count sort options, so this is fetched once and
/// cached UI-side.
pub async fn get_artists(state: &AppState) -> Result<Vec<artist::ArtistStats>, AppError> {
    queries::artist::get_all_artists(&state.db).await
}

/// Stats for a single artist, or `AppError::NotFound` if the id is gone.
pub async fn get_artist_detail(state: &AppState, id: i64) -> Result<artist::ArtistStats, AppError> {
    queries::artist::get_artist_by_id(&state.db, id).await
}

/// All albums by the given artist, in the same `AlbumStats` shape the
/// Albums grid uses — feeds the "Albums" sub-section in the Artist Detail
/// view's header.
pub async fn get_artist_albums(
    state: &AppState,
    artist_id: i64,
) -> Result<Vec<album::AlbumStats>, AppError> {
    queries::album::get_albums_by_artist(&state.db, artist_id).await
}

/// Every track credited to the artist, in the list-view projection the
/// shared `TrackList` component renders. Default order is the natural
/// `sort_key` (title) — the UI re-sorts in memory for the visible sort pill.
pub async fn get_artist_tracks(
    state: &AppState,
    artist_id: i64,
) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_tracks_by_artist_for_list(&state.db, artist_id).await
}
