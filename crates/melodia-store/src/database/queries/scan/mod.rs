//! Scan-time database operations: foreign-key upserts, track row
//! mutations, lookups, and the natural-sort-key helper.
//!
//! The four submodules are organised by *intent*: [`upserts`] handles
//! find-or-create on artist / album / genre rows, [`mutations`] holds
//! every track-row write, [`lookups`] holds read-side queries used by
//! the scanner / file-event processor, and [`sort_key`] precomputes the
//! `tracks.sort_key` column. Every public function is re-exported here
//! so callers continue to address them through `queries::scan::*`.

mod lookups;
mod mutations;
mod sort_key;
mod upserts;

pub use lookups::{
    find_folder_for_path, get_all_track_paths_for_folder, get_existing_track_summaries_for_folder,
    get_track_id_by_path, track_exists_by_path,
};
pub use mutations::{
    INSERT_CHUNK_ROWS, NewTrackRow, delete_track_by_path, delete_tracks_by_paths_batch,
    insert_track, insert_tracks_batch, prune_orphans, update_album_artwork_from_tracks,
    update_track_artwork_if_missing, update_track_location, update_track_metadata,
};
pub use sort_key::to_natural_sort_key;
pub use upserts::{
    CreditDetails, album_artist_name_for, album_credit_for, upsert_album, upsert_artist,
    upsert_genre,
};

/// Resolved foreign-key IDs for a track being inserted during a scan.
#[derive(Clone, Copy)]
pub struct ResolvedIds {
    pub artist_id: i64,
    pub album_id: Option<i64>,
    pub genre_id: Option<i64>,
    pub folder_id: i64,
}

/// Resolve a path's library-folder + artist/album/genre rows, upserting any
/// missing rows. Returns `None` (with a debug log under `context`) when the
/// path is not inside any library folder — callers should short-circuit.
///
/// Shared by the file-event reconcile path and the tag-edit orchestrator: both
/// need the same folder lookup + FK upsert sequence before writing a track row.
pub async fn resolve_track_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &std::path::Path,
    path_str: &str,
    meta: &melodia_core::entities::scan::ExtractedMetadata,
    context: &str,
) -> Result<Option<ResolvedIds>, melodia_core::error::AppError> {
    let Some(folder_id) = find_folder_for_path(tx, path_str).await? else {
        log::debug!("{context} file not in any library folder, skipping: {}", path.display());
        return Ok(None);
    };

    // The **first** credited name, not the whole credit line: an `artists` row named
    // "X feat. Y" is the bug this replaced, and it was one every multi-artist file created.
    let artist_name = meta.artist.primary_name();
    let album_name = meta.album.as_deref().unwrap_or("");
    // The first genre for the same reason: `tracks.genre_id` is the primary one, and the rest
    // reach the library through `track_genres`.
    let genre_name = meta.genres.primary().unwrap_or("");

    let artist_id = upsert_artist(tx, artist_name, 1).await?;
    // Group the album by its album-artist (falling back to the track artist when no
    // album-artist tag is present) so a per-track featured credit ("X & Y") doesn't
    // split the album into a second row.
    let album_artist_name = album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        upsert_artist(tx, album_artist_name, 1).await?
    };
    let album_credit = album_credit_for(meta);
    let album_id = upsert_album(tx, album_name, album_artist_id, &album_credit, meta).await?;
    let genre_id = upsert_genre(tx, genre_name).await?;

    Ok(Some(ResolvedIds {
        artist_id,
        album_id,
        genre_id,
        folder_id,
    }))
}

#[cfg(test)]
#[path = "../tests/scan_tests.rs"]
mod tests;
