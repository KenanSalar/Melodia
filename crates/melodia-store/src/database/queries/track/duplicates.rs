//! The duplicate finder's two reads: every track's key, then the facts of the candidates the keys
//! leave (`library::duplicates`).

use futures_util::TryStreamExt;

use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::duplicates::DuplicateCopy;
use melodia_core::error::AppError;

/// What a track's song key is made from.
#[derive(sqlx::FromRow)]
pub struct DuplicateKeyRow {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
}

/// Folds each track into `init` through `step` one row at a time, so the library's titles are never
/// all held at once. A zero length is one the decoder couldn't count either, which matches nothing.
pub async fn fold_duplicate_keys<A: Send + 'static>(
    db: &DbPool,
    init: A,
    mut step: impl FnMut(&mut A, DuplicateKeyRow) + Send + 'static,
) -> Result<A, AppError> {
    let mut rows = sqlx::query_as::<_, DuplicateKeyRow>(
        "SELECT id, title, artist FROM tracks WHERE duration_ms > 0",
    )
    .fetch(db.read());

    let mut acc = init;
    while let Some(row) = rows.try_next().await? {
        step(&mut acc, row);
    }
    Ok(acc)
}

/// The candidates' facts, each with its Not Duplicates set, in no particular order. The dialog's
/// rows read it too, by the ids on screen.
pub async fn duplicate_copies(db: &DbPool, ids: &[i64]) -> Result<Vec<DuplicateCopy>, AppError> {
    chunked_in_query(db.read(), ids, |placeholders| {
        format!(
            "SELECT t.id, t.file_path, t.file_hash, t.file_size, t.title, t.artist, t.album, \
                    t.disc_number, t.track_number, t.year, t.musicbrainz_track_id, \
                    t.musicbrainz_release_id, t.duration_ms, t.date_added, \
                    t.codec, t.bitrate, t.sample_rate, t.bit_depth, d.set_id AS dismissal, \
                    t.play_count, t.rating, t.is_favorite, \
                    (SELECT COUNT(*) FROM playlist_items p WHERE p.track_id = t.id) \
                        AS playlist_count \
             FROM tracks t LEFT JOIN duplicate_dismissals d ON d.track_id = t.id \
             WHERE t.id IN ({placeholders})"
        )
    })
    .await
}
