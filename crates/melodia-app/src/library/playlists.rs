use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::import::import_and_summarize;
use crate::state::AppState;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::entities::{playlist, track};
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Creates a playlist already holding `track_ids`, and returns its id.
pub async fn create_playlist(
    db: &DbPool,
    name: String,
    description: Option<String>,
    track_ids: Vec<i64>,
) -> Result<i64, AppError> {
    queries::playlist::create_playlist_with_tracks(db, &name, description.as_deref(), &track_ids)
        .await
}

pub async fn get_playlists(db: &DbPool) -> Result<Vec<playlist::PlaylistStats>, AppError> {
    queries::playlist::get_all_playlists(db).await
}

pub async fn get_playlist_detail(
    db: &DbPool,
    id: i64,
) -> Result<playlist::PlaylistStats, AppError> {
    queries::playlist::get_playlist_by_id(db, id).await
}

pub async fn get_playlist_tracks(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::playlist::get_playlist_tracks_for_list(db, playlist_id).await
}

pub async fn update_playlist(
    db: &DbPool,
    id: i64,
    name: String,
    description: Option<String>,
    clear_thumbnail: Option<bool>,
) -> Result<playlist::Playlist, AppError> {
    queries::playlist::update_playlist(
        db,
        id,
        &name,
        description.as_deref(),
        clear_thumbnail.unwrap_or(false),
    )
    .await
}

/// Drop one or more playlists. The single-card arm passes a one-element slice rather than
/// keeping a second spelling of the same statement.
pub async fn delete_playlists(db: &DbPool, ids: &[i64]) -> Result<(), AppError> {
    queries::playlist::delete_playlists(db, ids).await
}

pub async fn add_to_playlist(
    db: &DbPool,
    playlist_id: i64,
    track_ids: Vec<i64>,
) -> Result<(), AppError> {
    queries::playlist::add_tracks_to_playlist(db, playlist_id, &track_ids).await
}

/// For the Add-to-Playlist picker dialog: per-playlist count of how many
/// of the given `track_ids` are already in that playlist. Playlists with
/// zero overlap are omitted from the map.
pub async fn count_tracks_in_playlists_for_selection(
    db: &DbPool,
    track_ids: Vec<i64>,
) -> Result<HashMap<i64, i64>, AppError> {
    queries::playlist::count_tracks_in_playlists_for_selection(db, &track_ids).await
}

pub async fn remove_tracks_from_playlist_batch(
    db: &DbPool,
    playlist_id: i64,
    track_ids: Vec<i64>,
) -> Result<(), AppError> {
    queries::playlist::remove_tracks_from_playlist_batch(db, playlist_id, &track_ids).await
}

pub async fn reorder_playlist(
    db: &DbPool,
    playlist_id: i64,
    from: i32,
    to: i32,
) -> Result<(), AppError> {
    queries::playlist::reorder_playlist_track(db, playlist_id, from, to).await
}

/// How many distinct covers the Edit-Artwork mosaic picker offers.
///
/// The picker's grid is a plain repeater, not a virtualized list, so each
/// candidate is a live tile with its own decoded cover — a playlist
/// spanning hundreds of albums would otherwise mount hundreds of them
/// into a 160 px window and thrash the cover LRU. The user picks at most
/// four, and the window shows about a dozen at a time, so this is far
/// more scrolling than the choice needs.
const MOSAIC_CANDIDATE_LIMIT: i64 = 60;

pub async fn get_playlist_artwork_paths(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<String>, AppError> {
    queries::playlist::get_playlist_artwork_paths(db, playlist_id, MOSAIC_CANDIDATE_LIMIT).await
}

pub async fn set_playlist_thumbnail(
    state: &AppState,
    playlist_id: i64,
    image_paths: Vec<String>,
) -> Result<playlist::Playlist, AppError> {
    compose_thumbnail(&state.db, &state.paths.artwork_dir, playlist_id, &image_paths).await
}

/// [`set_playlist_thumbnail`]'s body, narrowed to what it reaches so the tests can drive the
/// 1-to-4 bound off a bare pool.
async fn compose_thumbnail(
    db: &DbPool,
    artwork_dir: &Path,
    playlist_id: i64,
    image_paths: &[String],
) -> Result<playlist::Playlist, AppError> {
    if image_paths.is_empty() || image_paths.len() > 4 {
        return Err(AppError::Validation("Must provide 1-4 image paths".to_owned()));
    }

    let source_paths: Vec<PathBuf> = image_paths.iter().map(PathBuf::from).collect();
    let composite_path =
        melodia_artwork::media::image::artwork::compose_artwork(&source_paths, artwork_dir)
            .ok_or_else(|| AppError::io_other("Failed to compose artwork"))?;

    queries::playlist::set_playlist_custom_thumbnail(db, playlist_id, &composite_path).await
}

#[derive(Clone, Serialize)]
pub struct ImportToPlaylistResult {
    pub imported_count: u32,
    pub added_count: u32,
    pub failed_paths: Vec<String>,
}

pub async fn import_files_to_playlist(
    state: &AppState,
    playlist_id: i64,
    file_paths: Vec<String>,
) -> Result<ImportToPlaylistResult, AppError> {
    import_into_playlist(
        &state.db,
        &state.paths.artwork_dir,
        &state.cover_cache,
        playlist_id,
        &file_paths,
    )
    .await
}

/// [`import_files_to_playlist`]'s body, narrowed the same way [`compose_thumbnail`] is.
async fn import_into_playlist(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    playlist_id: i64,
    file_paths: &[String],
) -> Result<ImportToPlaylistResult, AppError> {
    // `import_and_summarize` gives us title metadata in the same
    // round-trip, so we can sort the dropped batch alphabetically
    // (natord-aware) before persisting playlist positions — matches
    // `queue_import_files`'s ordering so a drop of "9.mp3, 10.mp3,
    // foo.mp3" always lands as "9, 10, foo" inside the playlist
    // instead of whatever order the filesystem / DB returned.
    let mut result = import_and_summarize(db, artwork_dir, cover_cache, file_paths).await?;

    let added_count = if result.summaries.is_empty() {
        0
    } else {
        let mut summaries = std::mem::take(&mut result.summaries);
        summaries.sort_by(|a, b| natord::compare(&a.title, &b.title));
        let sorted_ids: Vec<i64> = summaries.iter().map(|s| s.id).collect();
        queries::playlist::add_tracks_to_playlist(db, playlist_id, &sorted_ids).await?;
        sorted_ids.len()
    };

    Ok(ImportToPlaylistResult {
        imported_count: result.imported_count,
        added_count: u32::try_from(added_count).unwrap_or(u32::MAX),
        failed_paths: result.failed_paths,
    })
}

#[cfg(test)]
#[path = "tests/playlists_tests.rs"]
mod tests;
