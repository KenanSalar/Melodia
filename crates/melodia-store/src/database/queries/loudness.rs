//! Loudness measured from files carrying no gain tags, played in the tags' place.
//!
//! **A measurement applies only while its key matches the track**: the whole-file hash and the cue
//! span it was taken over. A replaced file stops matching once its re-read commits, and a track
//! whose cue timing moved once its sheet is read, with no write path owing an invalidation.
//! Melodia's own tag writes are the exception, changing the hash, not the audio ([`carry_over`]).

use rusqlite::Transaction;

use crate::database::sql;
use crate::database::{DbPool, chunked_in_query};
use melodia_core::error::AppError;

/// Joins the track `t` to its measurement `l` where the measurement still matches the audio and
/// the file carries neither gain, so `l` is NULL wherever the tags apply. The span compares with
/// `IS`, being NULL on a whole file and its end on a sheet's last track.
pub(crate) const MATCHING_MEASUREMENT: &str = "LEFT JOIN track_loudness l ON l.track_id = t.id \
    AND l.file_hash = t.file_hash AND l.cue_start IS t.cue_start AND l.cue_end IS t.cue_end \
    AND t.replaygain_track_gain IS NULL AND t.replaygain_album_gain IS NULL";

/// Moves the measurements of `path`'s rows taken at `old_hash` onto the hash the rows now carry,
/// once they have read back a file Melodia wrote only tags into: left alone, every rated track's
/// album is decoded again. Never from the general ingest, where a tag write and a replaced file
/// look the same.
pub(crate) fn carry_over(tx: &Transaction<'_>, path: &str, old_hash: &str) -> Result<(), AppError> {
    sql::execute(
        tx,
        "UPDATE track_loudness
         SET file_hash = (SELECT file_hash FROM tracks WHERE id = track_loudness.track_id)
         WHERE file_hash = ?2 AND track_id IN (SELECT id FROM tracks WHERE file_path = ?1)",
        (path, old_hash),
    )?;
    Ok(())
}

/// A file's hash as its rows recorded it before Melodia rewrote its tags, for
/// [`carry_over_rewritten`] once a read-back outside one transaction has committed.
pub struct HashOnRecord {
    path: String,
    hash: String,
}

/// The hash on record for each of `paths` whose rows have one.
pub async fn hashes_on_record(
    db: &DbPool,
    paths: Vec<String>,
) -> Result<Vec<HashOnRecord>, AppError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String)> = db
        .read(move |conn| {
            chunked_in_query(conn, &paths, |placeholders| {
                format!(
                    "SELECT DISTINCT file_path, file_hash FROM tracks \
                     WHERE file_hash IS NOT NULL AND file_path IN ({placeholders})"
                )
            })
        })
        .await?;
    Ok(rows.into_iter().map(|(path, hash)| HashOnRecord { path, hash }).collect())
}

/// [`carry_over`] for each file in `before`, in one transaction.
pub async fn carry_over_rewritten(db: &DbPool, before: Vec<HashOnRecord>) -> Result<(), AppError> {
    if before.is_empty() {
        return Ok(());
    }
    db.transaction(move |tx| {
        for file in &before {
            carry_over(tx, &file.path, &file.hash)?;
        }
        Ok(())
    })
    .await
}
