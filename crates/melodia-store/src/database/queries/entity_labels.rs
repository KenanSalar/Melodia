//! The names behind a set of entity ids, for the card menus' Copy entries.
//!
//! Batched and narrow, `track::entity_ids`' shape: a selection of cards is one statement per kind,
//! where a `get_*_by_id` per card would fetch a whole stats row to read one column of it. Rows come
//! back in whatever order the database picks, and restoring the order asked in is the caller's.

use crate::database::{DbPool, chunked_in_query};
use melodia_core::error::AppError;

/// `(id, name, artist)` per album. Off `album_stats` so the artist is the credit the album grid
/// prints, its `COALESCE` over the album's own credit staying spelled in the view alone.
pub async fn album_labels(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<(i64, String, String)>, AppError> {
    chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT id, name, artist_name FROM album_stats WHERE id IN ({placeholders})")
    })
    .await
}

pub async fn artist_names(db: &DbPool, ids: &[i64]) -> Result<Vec<(i64, String)>, AppError> {
    chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT id, name FROM artists WHERE id IN ({placeholders})")
    })
    .await
}

pub async fn genre_names(db: &DbPool, ids: &[i64]) -> Result<Vec<(i64, String)>, AppError> {
    chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT id, name FROM genres WHERE id IN ({placeholders})")
    })
    .await
}

pub async fn playlist_names(db: &DbPool, ids: &[i64]) -> Result<Vec<(i64, String)>, AppError> {
    chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT id, name FROM playlists WHERE id IN ({placeholders})")
    })
    .await
}

#[cfg(test)]
#[path = "tests/entity_labels_tests.rs"]
mod tests;
