//! The files opened from outside every library folder, each filed under a disabled folder made for
//! its directory ([`scan::TrackFolder::Outside`]). Such a row links to no album, artist or genre,
//! so no library page counts it, and it stays only while a playlist or the queue holds it.

use rusqlite::Transaction;

use crate::database::queries::scan;
use crate::database::queries::track::json_array;
use crate::database::{DbPool, chunked_in_execute, sql};
use melodia_core::error::AppError;

/// `statement` over `swept`: the folders whose ids `?1` lists as a JSON array, less any enabled
/// since the caller read them. `swept_rows` is their tracks, read folder first for
/// `file_rows::OUTSIDE_IDS`'s reason.
macro_rules! over_swept {
    ($statement:literal) => {
        concat!(
            "WITH swept AS (SELECT f.id FROM folders f JOIN json_each(?1) j ON j.value = f.id \
             WHERE NOT f.is_enabled), \
             swept_rows AS (SELECT t.id, t.file_path FROM swept \
             CROSS JOIN tracks t ON t.folder_id = swept.id) ",
            $statement
        )
    };
}

/// Holds the rows under the outside `folders` to that rule: unlinks any still linked, then deletes
/// the files none of whose rows a playlist or `queue` holds, then the folders left empty, cue sheets
/// and all, then the albums, artists and genres left with no track. Every row of a path goes
/// together, so a cue sheet's image is never half deleted.
pub async fn sweep(db: &DbPool, folders: &[i64], queue: &[i64]) -> Result<(), AppError> {
    if folders.is_empty() {
        return Ok(());
    }
    let folders = json_array(folders);
    let queue = json_array(queue);
    db.transaction(move |tx| {
        let unlinked = unlink_linked(tx, &folders)?;
        let deleted = sql::execute(
            tx,
            over_swept!(
                "DELETE FROM tracks WHERE id IN (SELECT id FROM swept_rows) \
                   AND file_path NOT IN ( \
                       SELECT held.file_path FROM swept_rows held \
                        WHERE EXISTS (SELECT 1 FROM playlist_items p WHERE p.track_id = held.id) \
                           OR held.id IN (SELECT value FROM json_each(?2)))"
            ),
            (folders.as_str(), queue.as_str()),
        )?;
        let emptied = sql::execute(
            tx,
            over_swept!(
                "DELETE FROM folders WHERE id IN (SELECT id FROM swept) \
                   AND NOT EXISTS (SELECT 1 FROM tracks t WHERE t.folder_id = folders.id)"
            ),
            (folders.as_str(),),
        )?;
        if unlinked || deleted > 0 {
            scan::prune_orphans(tx)?;
        }
        if deleted > 0 || emptied > 0 {
            log::info!(
                "Removed {deleted} track(s) opened from outside the library that nothing holds, \
                 and {emptied} folder(s) they left empty"
            );
        }
        Ok(())
    })
    .await
}

/// Unlinks every row under the swept folders that still carries a link, answering whether any did.
/// Each is logged, so a path that keeps linking shows up rather than being quietly undone.
fn unlink_linked(tx: &Transaction<'_>, folders: &str) -> Result<bool, AppError> {
    let linked: Vec<(i64, String)> = sql::fetch_all(
        tx,
        over_swept!(
            "SELECT t.id, t.file_path FROM swept_rows s JOIN tracks t ON t.id = s.id \
              WHERE t.artist_id IS NOT NULL OR t.album_id IS NOT NULL OR t.genre_id IS NOT NULL \
                 OR EXISTS (SELECT 1 FROM track_artists j WHERE j.track_id = t.id) \
                 OR EXISTS (SELECT 1 FROM track_genres j WHERE j.track_id = t.id) \
                 OR EXISTS (SELECT 1 FROM track_credits j WHERE j.track_id = t.id)"
        ),
        (folders,),
    )?;
    for (_, path) in &linked {
        log::warn!("Unlinked a track opened from outside the library: {path}");
    }
    let ids: Vec<i64> = linked.into_iter().map(|(id, _)| id).collect();
    unlink_on(tx, &ids)?;
    Ok(!ids.is_empty())
}

/// Clears the album, artist and genre links of the rows `ids`, join rows included, the stats
/// triggers counting each out of what it left.
pub(crate) fn unlink_on(tx: &Transaction<'_>, ids: &[i64]) -> Result<(), AppError> {
    chunked_in_execute(tx, ids, |id_list| {
        format!(
            "UPDATE tracks SET album_id = NULL, artist_id = NULL, genre_id = NULL \
             WHERE id IN ({id_list})"
        )
    })?;
    for table in ["track_artists", "track_genres", "track_credits"] {
        chunked_in_execute(tx, ids, |id_list| {
            format!("DELETE FROM {table} WHERE track_id IN ({id_list})")
        })?;
    }
    Ok(())
}
