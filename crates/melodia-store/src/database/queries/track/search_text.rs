//! Filling `tracks.search_text` on the rows ingested before the column existed.

use crate::database::{DbPool, FoldedColumn, FoldedInputs, backfill_folded};
use melodia_core::entities::track_search::SearchFields;
use melodia_core::error::AppError;

const SEARCH_TEXT: FoldedColumn = FoldedColumn {
    table: "tracks",
    column: "search_text",
    inputs: "SELECT id, title, artist, album_artist, album, genre FROM tracks x",
};

/// The five columns a row's search text is packed from.
#[derive(sqlx::FromRow)]
struct Unfilled {
    id: i64,
    title: String,
    artist: Option<String>,
    album_artist: Option<String>,
    album: Option<String>,
    genre: Option<String>,
}

impl FoldedInputs for Unfilled {
    fn id(&self) -> i64 {
        self.id
    }

    fn packed(&self) -> String {
        SearchFields {
            title: &self.title,
            artist: self.artist.as_deref().unwrap_or_default(),
            album_artist: self.album_artist.as_deref().unwrap_or_default(),
            album: self.album.as_deref().unwrap_or_default(),
            genre: self.genre.as_deref().unwrap_or_default(),
        }
        .pack()
    }
}

/// Fills `search_text` on every row still holding `''`, returning how many rows it wrote
/// ([`backfill_folded`]).
pub async fn backfill_search_text(db: &DbPool) -> Result<u64, AppError> {
    backfill_folded::<Unfilled>(db, &SEARCH_TEXT).await
}
