//! The database half of an Edit Track Information save: the rows of files whose tags were already
//! rewritten and read back, refreshed in one transaction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    NameCache, ResolvedIds, prune_orphans, resolve_track_context, update_album_artwork_from_tracks,
    update_track_metadata,
};
use crate::database::DbPool;
use crate::database::queries::{album, playlist, track};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::tags::{ArtworkEdit, TagEdit};
use melodia_core::error::AppError;

/// A file whose tags were rewritten, with what reading it back found.
pub struct RetaggedFile<'a> {
    pub id: i64,
    pub path: &'a str,
    pub meta: &'a ExtractedMetadata,
}

/// What became of one [`RetaggedFile`]'s row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retag {
    Updated,
    /// The file sits in no library folder, so its row was left as it was.
    OutsideLibrary,
}

/// [`commit_retag`]'s memo key: everything [`resolve_track_context`] derives its ids from, the
/// folder by way of the parent directory, and `year` because the album upsert coalesces it.
///
/// **Ids only**: a hit skips `upsert_album`'s release columns, so the first file to resolve a key
/// writes them. Right for the dialog, where every selected file receives the same values.
type ResolveKey = (PathBuf, String, String, String, Option<i32>, String);

/// Resolves ids for each file, refreshes its row and applies the artwork override the metadata
/// UPDATE can't express, answering in `files`' order.
pub async fn commit_retag(
    db: &DbPool,
    files: &[RetaggedFile<'_>],
    edit: &TagEdit,
    cached_artwork: Option<&str>,
) -> Result<Vec<Retag>, AppError> {
    let mut tx = db.write().begin().await?;
    let mut outcomes: Vec<Retag> = Vec::with_capacity(files.len());
    let mut updated_ids: Vec<i64> = Vec::new();
    let mut album_ids: Vec<i64> = Vec::new();
    // A whole-album batch resolves each distinct key once.
    let mut resolve_cache: HashMap<ResolveKey, ResolvedIds> = HashMap::new();
    // The credit tables ask per credited name, which `ResolveKey` doesn't cover.
    let mut names = NameCache::for_chunk(files.len());
    // Remove's ids left with no cover at all, nulled in one statement after the loop.
    let mut remove_null_ids: Vec<i64> = Vec::new();
    // The release fields `upsert_album` can't empty, and the albums owed the statement that can.
    let cleared_release = edit.cleared_release_tags();
    let mut cleared_album_ids: Vec<i64> = Vec::new();

    for file in files {
        let path = Path::new(file.path);
        let meta = file.meta;
        // The **first** credited names, as `resolve_track_context` reads them, or a hit answers
        // with ids it would never produce.
        let key: ResolveKey = (
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
            meta.artist.primary_name().to_owned(),
            meta.album.clone().unwrap_or_default(),
            meta.album_artist.primary_name().to_owned(),
            meta.year,
            meta.genres.primary().unwrap_or_default().to_owned(),
        );
        let rids = if let Some(cached) = resolve_cache.get(&key) {
            *cached
        } else {
            let Some(resolved) =
                resolve_track_context(&mut tx, path, file.path, meta, "Tag edit", &mut names)
                    .await?
            else {
                outcomes.push(Retag::OutsideLibrary);
                continue;
            };
            resolve_cache.insert(key, resolved);
            resolved
        };

        update_track_metadata(&mut tx, file.path, meta, &rids, &mut names).await?;
        updated_ids.push(file.id);
        outcomes.push(Retag::Updated);

        if !cleared_release.is_empty()
            && let Some(aid) = rids.album_id
            && !cleared_album_ids.contains(&aid)
        {
            cleared_album_ids.push(aid);
        }

        // What the metadata UPDATE can't express: its `COALESCE` never nulls a path, and a
        // Replaced file re-extracts to the *external* cover, which shadows the one just embedded.
        match edit.artwork {
            ArtworkEdit::Replace => {
                if let Some(aid) = rids.album_id {
                    album_ids.push(aid);
                }
            }
            ArtworkEdit::Remove => {
                // The album keeps its cover. A track that re-extracted an external cover already
                // carries it, so only the nulls need a statement.
                if meta.artwork_path.is_none() {
                    remove_null_ids.push(file.id);
                }
            }
            ArtworkEdit::Keep => {}
        }
    }

    match edit.artwork {
        ArtworkEdit::Replace => {
            apply_replace_artwork(&mut tx, &updated_ids, &mut album_ids, cached_artwork).await?;
            playlist::refresh_automatic_thumbnails(&mut tx).await?;
        }
        ArtworkEdit::Remove => {
            if !remove_null_ids.is_empty() {
                track::set_track_artwork(&mut tx, &remove_null_ids, None).await?;
            }
            playlist::refresh_automatic_thumbnails(&mut tx).await?;
        }
        ArtworkEdit::Keep => {}
    }

    // After every `upsert_album` above, which is what it exists to undo.
    album::clear_release_tags(&mut tx, &cleared_album_ids, cleared_release).await?;

    // Both passes answer to a track that changed parents and sweep whole tables, which a rating
    // click shouldn't pay for. The residue: a file whose tags had drifted from its row can be
    // re-homed by the re-extract, stranding its old parent and leaving the new one coverless until
    // the next scan or watcher batch that writes.
    if edit.moves_between_parents() {
        // Null-only: a track retagged into another album hands it its cover.
        update_album_artwork_from_tracks(&mut tx).await?;
        prune_orphans(&mut tx).await?;
    }

    tx.commit().await?;
    Ok(outcomes)
}

/// Overwrites every updated track's and album's cover with the one just cached, the roll-up only
/// filling empty ones. Does nothing where the cache write failed: keeping the old cover beats
/// nulling one that can't be re-pointed.
async fn apply_replace_artwork(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    updated_ids: &[i64],
    album_ids: &mut Vec<i64>,
    cached_artwork: Option<&str>,
) -> Result<(), AppError> {
    if updated_ids.is_empty() {
        return Ok(());
    }
    let Some(cached) = cached_artwork else {
        return Ok(());
    };
    track::set_track_artwork(tx, updated_ids, Some(cached)).await?;
    album_ids.sort_unstable();
    album_ids.dedup();
    album::set_album_artwork(tx, album_ids, Some(cached)).await?;
    Ok(())
}
