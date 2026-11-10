//! The database half of an Edit Track Information save: the rows of files whose tags were already
//! rewritten and read back, refreshed in one transaction.

use std::collections::HashMap;

use rusqlite::Transaction;

use super::{
    NameCache, ResolvedIds, prune_orphans, resolve_track_context, update_album_artwork_from_tracks,
    update_track_metadata,
};
use crate::database::queries::{album, playlist, track};
use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::tags::{ArtworkEdit, ClearedReleaseTags, TagEdit};
use melodia_core::error::AppError;

/// A file whose tags were rewritten, with what reading it back found.
pub struct RetaggedFile {
    pub id: i64,
    pub path: String,
    pub meta: ExtractedMetadata,
}

/// What became of one [`RetaggedFile`]'s row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retag {
    Updated,
    /// The row left the library while its file was written, so there was nothing to refresh.
    Gone,
}

/// [`commit_retag`]'s memo key: everything [`resolve_track_context`] derives its ids from, `year`
/// because the album upsert coalesces it, and the row's folder, which a hit hands back as the
/// `folder_id` the update writes.
///
/// **Ids only**: a hit skips `upsert_album`'s release columns, so the first file to resolve a key
/// writes them. Right for the dialog, where every selected file receives the same values.
type ResolveKey = (i64, String, String, String, Option<i32>, String);

/// Resolves ids for each file, refreshes its row and applies the artwork override the metadata
/// UPDATE can't express, answering in `files`' order.
pub async fn commit_retag(
    db: &DbPool,
    files: Vec<RetaggedFile>,
    edit: &TagEdit,
    cached_artwork: Option<String>,
) -> Result<Vec<Retag>, AppError> {
    let edit = EditFacts {
        artwork: edit.artwork.clone(),
        cleared_release: edit.cleared_release_tags(),
        moves_between_parents: edit.moves_between_parents(),
        cached_artwork,
    };
    db.transaction(move |tx| refresh_rows(tx, &files, &edit)).await
}

/// What the commit needs from its [`TagEdit`] and the cached cover, owned by the transaction.
struct EditFacts {
    artwork: ArtworkEdit,
    cleared_release: ClearedReleaseTags,
    moves_between_parents: bool,
    cached_artwork: Option<String>,
}

fn refresh_rows(
    tx: &Transaction<'_>,
    files: &[RetaggedFile],
    edit: &EditFacts,
) -> Result<Vec<Retag>, AppError> {
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
    let cleared_release = edit.cleared_release;
    let mut cleared_album_ids: Vec<i64> = Vec::new();
    let folder_of = folder_ids(tx, files)?;

    for file in files {
        // A tag edit moves no file, so the row stays under its folder, an import's included.
        let Some(&folder_id) = folder_of.get(&file.id) else {
            outcomes.push(Retag::Gone);
            continue;
        };
        let meta = &file.meta;
        // The **first** credited names, as `resolve_track_context` reads them, or a hit answers
        // with ids it would never produce.
        let key: ResolveKey = (
            folder_id,
            meta.artist.primary_name().to_owned(),
            meta.album.clone().unwrap_or_default(),
            meta.album_artist.primary_name().to_owned(),
            meta.year,
            meta.genres.primary().unwrap_or_default().to_owned(),
        );
        let rids = if let Some(cached) = resolve_cache.get(&key) {
            *cached
        } else {
            let resolved = resolve_track_context(tx, folder_id, meta, &mut names)?;
            resolve_cache.insert(key, resolved);
            resolved
        };

        if !update_track_metadata(tx, &file.path, meta, &rids, &mut names)? {
            outcomes.push(Retag::Gone);
            continue;
        }
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
            let cached_artwork = edit.cached_artwork.as_deref();
            apply_replace_artwork(tx, &updated_ids, &mut album_ids, cached_artwork)?;
            playlist::refresh_automatic_thumbnails(tx)?;
        }
        ArtworkEdit::Remove => {
            if !remove_null_ids.is_empty() {
                track::set_track_artwork(tx, &remove_null_ids, None)?;
            }
            playlist::refresh_automatic_thumbnails(tx)?;
        }
        ArtworkEdit::Keep => {}
    }

    // After every `upsert_album` above, which is what it exists to undo.
    album::clear_release_tags(tx, &cleared_album_ids, cleared_release)?;

    // Both passes answer to a track that changed parents and sweep whole tables, which a rating
    // click shouldn't pay for. The residue: a file whose tags had drifted from its row can be
    // re-homed by the re-extract, stranding its old parent and leaving the new one coverless until
    // the next scan or watcher batch that writes.
    if edit.moves_between_parents {
        // Null-only: a track retagged into another album hands it its cover.
        update_album_artwork_from_tracks(tx)?;
        prune_orphans(tx)?;
    }

    Ok(outcomes)
}

/// `track id → folder_id` of the rows behind `files`. A row deleted since the edit began has none.
fn folder_ids(tx: &Transaction<'_>, files: &[RetaggedFile]) -> Result<HashMap<i64, i64>, AppError> {
    let ids: Vec<i64> = files.iter().map(|file| file.id).collect();
    let rows: Vec<(i64, i64)> = chunked_in_query(tx, &ids, |placeholders| {
        format!("SELECT id, folder_id FROM tracks WHERE id IN ({placeholders})")
    })?;
    Ok(rows.into_iter().collect())
}

/// Overwrites every updated track's and album's cover with the one just cached, the roll-up only
/// filling empty ones. Does nothing where the cache write failed: keeping the old cover beats
/// nulling one that can't be re-pointed.
fn apply_replace_artwork(
    tx: &Transaction<'_>,
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
    track::set_track_artwork(tx, updated_ids, Some(cached))?;
    album_ids.sort_unstable();
    album_ids.dedup();
    album::set_album_artwork(tx, album_ids, Some(cached))?;
    Ok(())
}
