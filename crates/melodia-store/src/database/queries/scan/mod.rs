//! Scan-time database operations: the three write transactions a library change lands in, and the
//! pieces they are built from.
//!
//! [`commit_scan`], [`commit_retag`] and [`apply_watch_batch`] each own their transaction, and are
//! the only writes here another crate can reach: every query taking a transaction is `pub(crate)`.

mod finish;
mod lookups;
mod mutations;
mod name_cache;
mod retag;
mod sort_key;
mod upserts;
mod watch_batch;

pub use finish::commit_scan;
pub use lookups::visit_folder_tracks;
pub(crate) use lookups::{find_folder_for_path, get_track_id_by_path, track_exists_by_path};
pub use mutations::{INSERT_CHUNK_ROWS, NewTrackRow};
pub(crate) use mutations::{
    delete_track_by_path, delete_tracks_by_paths_batch, insert_track, insert_tracks_batch,
    prune_orphans, roll_up_covers, update_album_artwork_from_tracks, update_track_location,
    update_track_metadata,
};
pub(crate) use name_cache::NameCache;
pub use retag::{Retag, RetaggedFile, commit_retag};
pub use sort_key::to_natural_sort_key;
pub(crate) use upserts::upsert_album;
pub use upserts::{CreditDetails, album_artist_name_for, album_credit_for};
pub use watch_batch::{WatchedChange, apply_watch_batch};

/// Resolved foreign-key IDs for a track being inserted during a scan.
#[derive(Clone, Copy)]
pub struct ResolvedIds {
    pub artist_id: i64,
    pub album_id: Option<i64>,
    pub genre_id: Option<i64>,
    pub folder_id: i64,
}

/// Resolves a path's library folder and its artist, album and genre rows, upserting what's
/// missing; `None`, logged under `context`, for a path outside every library folder.
pub(crate) async fn resolve_track_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &std::path::Path,
    path_str: &str,
    meta: &melodia_core::entities::scan::ExtractedMetadata,
    context: &str,
    names: &mut NameCache,
) -> Result<Option<ResolvedIds>, melodia_core::error::AppError> {
    let Some(folder_id) = find_folder_for_path(tx, path_str).await? else {
        log::debug!("{context} file not in any library folder, skipping: {}", path.display());
        return Ok(None);
    };

    // The **first** credited name: a row named "X feat. Y" is an artist nobody recorded under.
    let artist_name = meta.artist.primary_name();
    let album_name = meta.album.as_deref().unwrap_or("");
    // The first genre too, the rest reaching the library through `track_genres`.
    let genre_name = meta.genres.primary().unwrap_or("");

    let artist_id = names.artist(tx, artist_name, 1).await?;
    // The album groups by its album artist, so a per-track featured credit doesn't split it.
    let album_artist_name = album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        names.artist(tx, album_artist_name, 1).await?
    };
    let album_credit = album_credit_for(meta);
    let album_id =
        upsert_album(tx, album_name, album_artist_id, &album_credit, meta, names).await?;
    let genre_id = names.genre(tx, genre_name).await?;

    Ok(Some(ResolvedIds { artist_id, album_id, genre_id, folder_id }))
}

#[cfg(test)]
#[path = "../tests/scan_tests.rs"]
mod tests;
