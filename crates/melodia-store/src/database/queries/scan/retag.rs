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

/// Cache key for [`commit_retag`]'s FK-resolution memo. Holds everything
/// [`resolve_track_context`] derives its [`ResolvedIds`] from —
/// folder (via the parent dir, since folder lookup is a path-prefix match),
/// artist, album, album-artist (the album's grouping key), `year` (album upsert's
/// `COALESCE`-on-conflict input), and genre — so identical keys yield identical
/// ids. Keeping `year` in the key preserves the per-track album-year semantics:
/// tracks with differing years land in different buckets and each still upserts.
///
/// **Ids only.** `upsert_album` also writes the release-level columns off the same metadata, and a
/// cache hit skips that write, so whichever file resolved the key first is the one those columns
/// come from. That is right for the dialog, where every selected file receives the same values,
/// and it is why a batch of files disagreeing about a label has no surface to show it on.
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
    // FK resolution is identical for every track sharing a folder + artist +
    // album/year + genre (the whole-album batch — the flagship case), so resolve
    // each distinct tuple once instead of re-running a folder lookup + three
    // `INSERT … ON CONFLICT … RETURNING` upserts per track. Function-scoped, so
    // it drops at batch end (no persistent cache).
    let mut resolve_cache: HashMap<ResolveKey, ResolvedIds> = HashMap::new();
    // The credit tables ask per credited name per track, which `ResolveKey` never covered: it
    // keys on the *primary* names, so a guest on every track of the selection resolved once per
    // file. Same scope, dropped at batch end.
    let mut names = NameCache::for_chunk(files.len());
    // Artwork-Remove ids with nothing left to point at, flushed as one `IN (…)` UPDATE after
    // the loop.
    let mut remove_null_ids: Vec<i64> = Vec::new();
    // The release-level fields `upsert_album`'s COALESCE cannot empty, and the albums owed the
    // statement that does. Answered once: an edit clearing none of them collects nothing.
    let cleared_release = edit.cleared_release_tags();
    let mut cleared_album_ids: Vec<i64> = Vec::new();

    for file in files {
        let path = Path::new(file.path);
        let meta = file.meta;
        // Mirror what `resolve_track_context` keys on, or the cache answers with ids it would
        // never have produced: the **first** credited name, not the whole credit line, since
        // that is the name the `artists` row carries.
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

        // Artwork the metadata UPDATE can't express: its `COALESCE(?, ...)` can
        // never null a path, and a re-extract of a Replaced file returns the
        // *external* cover (which shadows embedded art) rather than the one we
        // just embedded.
        match edit.artwork {
            ArtworkEdit::Replace => {
                if let Some(aid) = rids.album_id {
                    album_ids.push(aid);
                }
            }
            ArtworkEdit::Remove => {
                // Album artwork is left alone: blanking a whole album because one track's
                // embedded art was removed would be wrong. A track whose re-extract found an
                // external `cover.jpg` already carries it, the metadata UPDATE's COALESCE
                // having just written it, so only the nulls need a statement of their own.
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

    // Both passes answer to a track that changed parents, and the sweep is whole-table:
    // three correlated deletes inside this transaction on a single-writer pool. A
    // rating write reaches here on one click and can move nothing, so gating them is
    // most of what that click costs.
    //
    // The residue is that a file whose tags had already drifted from the database
    // can be re-homed by the re-extract above: its old parent is left stranded, and
    // the album row it lands in instead gets no cover. The next scan or watcher batch
    // that writes a row runs both passes and repairs both.
    if edit.moves_between_parents() {
        // Null-only, never an overwrite: a track retagged into another album hands that album
        // its cover, as a scan, an import or a watcher batch would.
        update_album_artwork_from_tracks(&mut tx).await?;

        // Retagging a track into a different album or genre can strand its old
        // album (and that album's artist) or old genre with zero tracks; nothing
        // else deletes an emptied row, so sweep orphans before committing.
        prune_orphans(&mut tx).await?;
    }

    tx.commit().await?;
    Ok(outcomes)
}

/// Replace: one authoritative overwrite to every updated track and its album(s),
/// so the Albums grid card updates too (the metadata roll-up only fills NULL
/// rows). No-op when nothing was updated, or when the cache write failed
/// (`cached_artwork` is `None`) — leaving the COALESCE'd value beats nulling a
/// cover we can't repoint.
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
