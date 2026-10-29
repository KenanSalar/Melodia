use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::import::import_and_summarize;
use crate::state::AppState;
use crate::tasks::artwork_sweep::ArtworkHold;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::entities::playlist::{self, PlaylistItemsCut, PlaylistSnapshot};
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::entities::undo::UndoCapture;
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

/// What a delete took, and the custom covers kept from the sweep until the undo goes.
pub struct DeletedPlaylists {
    snapshots: Vec<PlaylistSnapshot>,
    covers: ArtworkHold,
}

impl DeletedPlaylists {
    /// A later delete, for a run merged into one undo.
    pub fn absorb(&mut self, newer: Self) {
        self.snapshots.extend(newer.snapshots);
        self.covers.absorb(newer.covers);
    }

    pub fn playlist_count(&self) -> usize {
        self.snapshots.len()
    }

    /// What the undo cap is checked on.
    pub fn item_count(&self) -> usize {
        self.snapshots.iter().map(|snapshot| snapshot.items.len()).sum()
    }

    pub fn newest_name(&self) -> &str {
        self.snapshots.last().map_or("", |snapshot| &snapshot.playlist.name)
    }
}

/// How many tracks deleting `ids` would take out of playlists, which decides whether the delete
/// can be undone.
pub async fn playlist_item_count(db: &DbPool, ids: Vec<i64>) -> Result<usize, AppError> {
    queries::playlist_delete::item_count(db, &ids).await
}

/// Deletes playlists, holding what puts them back, or answers `None` where none was there. Its
/// caller checks [`playlist_item_count`] against `UNDO_HOLD_CAP` first.
pub async fn delete_playlists_held(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Option<DeletedPlaylists>, AppError> {
    let covers = ArtworkHold::new(queries::playlist_delete::custom_covers(db, &ids).await?);
    let snapshots = queries::playlist_delete::delete_playlists_held(db, &ids).await?;
    Ok((!snapshots.is_empty()).then_some(DeletedPlaylists { snapshots, covers }))
}

/// Deletes playlists for good, answering the names of those that went.
pub async fn delete_playlists(db: &DbPool, ids: Vec<i64>) -> Result<Vec<String>, AppError> {
    queries::playlist_delete::delete_playlists(db, &ids).await
}

/// Undoes a delete. The covers stay held until the rows naming them are back.
pub async fn restore_playlists(db: &DbPool, deleted: DeletedPlaylists) -> Result<(), AppError> {
    let DeletedPlaylists { snapshots, covers } = deleted;
    queries::playlist_restore::restore_playlists(db, &snapshots).await?;
    drop(covers);
    Ok(())
}

pub async fn add_to_playlist(
    db: &DbPool,
    playlist_id: i64,
    track_ids: Vec<i64>,
) -> Result<(), AppError> {
    queries::playlist::add_tracks_to_playlist(db, playlist_id, &track_ids).await
}

/// For the Add-to-Playlist picker: how many of `track_ids` each playlist already holds, playlists
/// with none omitted.
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
) -> Result<UndoCapture<PlaylistItemsCut>, AppError> {
    queries::playlist::remove_tracks_from_playlist_batch(db, playlist_id, &track_ids).await
}

/// Undoes a run of removals from one playlist, `cuts` in the order they were made.
pub async fn restore_playlist_items(
    db: &DbPool,
    cuts: Vec<PlaylistItemsCut>,
) -> Result<(), AppError> {
    queries::playlist_restore::restore_playlist_items(db, &cuts).await
}

pub async fn move_playlist_tracks(
    db: &DbPool,
    playlist_id: i64,
    track_ids: Vec<i64>,
    to: usize,
) -> Result<(), AppError> {
    queries::playlist::move_playlist_tracks(db, playlist_id, &track_ids, to).await
}

/// How many distinct covers the Edit-Artwork mosaic picker offers. Its grid is a plain repeater,
/// each candidate a live tile with a decoded cover, so a playlist spanning hundreds of albums would
/// thrash the cover LRU, for a pick of at most four.
const MOSAIC_CANDIDATE_LIMIT: i64 = 60;

/// The covers the Edit-Artwork picker offers for the playlist `scope` lists.
pub async fn mosaic_candidates(
    db: &DbPool,
    scope: TrackListScope,
) -> Result<Vec<String>, AppError> {
    queries::track::cover_paths(db, scope, MOSAIC_CANDIDATE_LIMIT).await
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
        return Err(AppError::validation_msg("Must provide 1-4 image paths"));
    }

    let source_paths: Vec<PathBuf> = image_paths.iter().map(PathBuf::from).collect();
    let composite_path =
        melodia_artwork::media::image::artwork::compose_artwork(&source_paths, artwork_dir)
            .ok_or_else(|| AppError::io_other("Failed to compose artwork"))?;

    queries::playlist::set_playlist_custom_thumbnail(db, playlist_id, &composite_path).await
}

#[derive(Clone)]
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
    // Sorted as `queue_import_files` sorts a drop, so "9.mp3, 10.mp3" lands as 9 then 10 rather
    // than in whatever order the filesystem returned.
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
