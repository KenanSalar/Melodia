//! Name-to-id answers held for the span of one write transaction.

use std::collections::HashMap;

use melodia_core::error::AppError;

use super::upserts::{upsert_artist, upsert_genre};

/// The artist and genre ids a transaction has already resolved, by name as spelled.
///
/// A credit repeats across a release and the join writers ask once per credit per track, so
/// without this a guest on a 200-track box set is 200 identical upserts. None of them is a read:
/// `ON CONFLICT DO UPDATE SET name = excluded.name` rewrites the row and its index entry to say
/// what it already said.
///
/// Keyed on the exact spelling. `artists.name` is `UNIQUE COLLATE NOCASE`, so two spellings share
/// one row and the upsert leaves the newest standing; caching by exact string keeps that and drops
/// only the repeats. The one thing it does change is that a spelling seen *again* after a
/// different one no longer wins the row back, which is how [`super::super::ingest`] has always
/// behaved for a track's primary artist.
///
/// Scoped to the caller's transaction and dropped with it, so it holds the distinct names of one
/// chunk and never the library's.
#[derive(Default)]
pub struct NameCache {
    artists: HashMap<String, i64>,
    genres: HashMap<String, Option<i64>>,
}

impl NameCache {
    /// Sized for a chunk of `files`, on the shape a music library tends to have.
    #[must_use]
    pub fn for_chunk(files: usize) -> Self {
        Self { artists: HashMap::with_capacity(files / 10 + 1), genres: HashMap::with_capacity(32) }
    }

    /// The artist row for `name`, upserting it the first time this transaction asks.
    ///
    /// Returns `unknown_artist_id` for an empty name, so callers stay branch-free the way
    /// [`upsert_artist`] lets them.
    pub async fn artist(
        &mut self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        name: &str,
        unknown_artist_id: i64,
    ) -> Result<i64, AppError> {
        if name.is_empty() {
            return Ok(unknown_artist_id);
        }
        if let Some(&id) = self.artists.get(name) {
            return Ok(id);
        }
        let id = upsert_artist(tx, name, unknown_artist_id).await?;
        self.artists.insert(name.to_owned(), id);
        Ok(id)
    }

    /// The genre row for `name`, `None` for an empty one.
    pub async fn genre(
        &mut self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        name: &str,
    ) -> Result<Option<i64>, AppError> {
        if name.is_empty() {
            return Ok(None);
        }
        if let Some(&id) = self.genres.get(name) {
            return Ok(id);
        }
        let id = upsert_genre(tx, name).await?;
        self.genres.insert(name.to_owned(), id);
        Ok(id)
    }
}
