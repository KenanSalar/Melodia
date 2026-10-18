//! Filling `tracks.search_text` on the rows ingested before the column existed.

use sqlx::AssertSqlSafe;

use crate::database::{
    CASE_BINDS_PER_ROW, DbPool, MAX_BINDS_PER_STATEMENT, case_by_id, placeholders,
};
use melodia_core::entities::track_search::SearchFields;
use melodia_core::error::AppError;

/// Rows per page, so that one page is one `UPDATE` inside the bind budget.
const PAGE_ROWS: usize = MAX_BINDS_PER_STATEMENT / CASE_BINDS_PER_ROW;

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

impl Unfilled {
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

/// Fills `search_text` on every row still holding `''`, a page at a time, and returns how many
/// rows it wrote.
///
/// Cheap to run on every launch: a filled library answers the first page from an empty partial
/// index. Each page is its own statement rather than one transaction around the pass, so a scan
/// waiting on the single writer waits for a page rather than the library. And the `UPDATE` asks
/// again that the column is empty, so a row a scan re-wrote between the read and the write keeps
/// the scan's value instead of one packed from the fields it replaced.
pub async fn backfill_search_text(db: &DbPool) -> Result<u64, AppError> {
    let page_limit = i64::try_from(PAGE_ROWS).unwrap_or(i64::MAX);
    let mut after_id = 0_i64;
    let mut written = 0;
    loop {
        let page: Vec<Unfilled> = sqlx::query_as(
            "SELECT id, title, artist, album_artist, album, genre FROM tracks \
             WHERE search_text = '' AND id > ? ORDER BY id LIMIT ?",
        )
        .bind(after_id)
        .bind(page_limit)
        .fetch_all(db.read())
        .await?;
        let Some(last) = page.last() else {
            return Ok(written);
        };
        after_id = last.id;
        written += fill_page(db, &page).await?;
    }
}

async fn fill_page(db: &DbPool, page: &[Unfilled]) -> Result<u64, AppError> {
    let sql = format!(
        "UPDATE tracks SET search_text = {} WHERE id IN ({}) AND search_text = ''",
        case_by_id(page.len()),
        placeholders(page.len()),
    );
    let mut query = sqlx::query(AssertSqlSafe(sql));
    for row in page {
        query = query.bind(row.id).bind(row.packed());
    }
    for row in page {
        query = query.bind(row.id);
    }
    Ok(query.persistent(false).execute(db.write()).await?.rows_affected())
}
