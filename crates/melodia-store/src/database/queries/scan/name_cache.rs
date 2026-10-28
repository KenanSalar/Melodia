//! Name-to-id answers held for the span of one write transaction.

use std::collections::HashMap;

use melodia_core::error::AppError;

use super::upserts::{upsert_artist, upsert_genre};

/// The artist and genre ids a transaction has already resolved, by name as spelled.
///
/// The join writers ask once per credit per track, and each upsert rewrites its row and index
/// entry to say what they already said. Keyed on the exact spelling: two spellings share one
/// `NOCASE` row with the newer standing, and one seen again after another doesn't win it back.
/// Dropped with the transaction, so it holds one chunk's names, never the library's.
#[derive(Default)]
pub(crate) struct NameCache {
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
