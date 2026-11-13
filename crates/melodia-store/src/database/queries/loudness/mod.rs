//! Loudness measured from files carrying no gain tags, played in the tags' place.
//!
//! **A measurement applies only while its key matches the track**: the whole-file hash and the cue
//! span it was taken over. A replaced file stops matching once its re-read commits, and a track
//! whose cue timing moved once its sheet is read, with no write path owing an invalidation.
//! Melodia's own tag writes are the exception, changing the hash, not the audio ([`carry_over`]).
//!
//! **An album's values sit on each member's row with the album and the member count they were
//! measured over**, so a read applies them only while the track is still on that album, and
//! [`settle`] can tell when the album changed under them. The count stands beside the id because a
//! pruned album's id can come back.

mod record;
mod work;

pub use record::*;
pub use work::*;

use rusqlite::Transaction;

use crate::database::rows::by_name;
use crate::database::sql;
use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::track::TrackAudio;
use melodia_core::error::AppError;

/// Whether the track aliased `$t` carries neither gain tag. Either one makes the file tagged, the
/// player falling back from one pair to the other.
macro_rules! untagged {
    ($t:ident) => {
        concat!(
            stringify!($t),
            ".replaygain_track_gain IS NULL AND ",
            stringify!($t),
            ".replaygain_album_gain IS NULL"
        )
    };
}
use untagged;

/// Joins the track `t` to its measurement `l` where the measurement still matches the audio and
/// the file carries neither gain, so `l` is NULL wherever the tags apply. The span compares with
/// `IS`, being NULL on a whole file and its end on a sheet's last track.
pub(crate) const MATCHING_MEASUREMENT: &str = concat!(
    "LEFT JOIN track_loudness l ON l.track_id = t.id \
     AND l.file_hash = t.file_hash AND l.cue_start IS t.cue_start AND l.cue_end IS t.cue_end AND ",
    untagged!(t)
);

/// A track a pass decodes, and the key its measurement is recorded against.
#[derive(Debug, Clone)]
pub struct MeasureTrack {
    pub id: i64,
    pub audio: TrackAudio,
    file_hash: String,
}

by_name!(MeasureTrack { id, file_hash, audio: TrackAudio });

/// A library album measured as one whole, its members in the order its page lists them.
#[derive(Debug, Clone)]
pub struct AlbumJob {
    pub album_id: i64,
    pub members: Vec<MeasureTrack>,
}

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

#[cfg(test)]
#[path = "../tests/loudness_tests.rs"]
mod tests;
