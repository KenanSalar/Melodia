//! The lowercase keys the Songs tabs' artist, album and genre orders read, and the fill that gives
//! a row written before they existed its keys.

use rusqlite::Transaction;

use crate::database::DbPool;
use crate::database::rows::by_name;
use crate::database::sql;
use melodia_core::error::AppError;

/// Rows the fill reads at a time, which bounds what it holds, not how often it commits.
const FILL_PAGE_ROWS: i64 = 1_000;

/// Every write fills the three keys together, so `artist_key`'s NULL stands for all three, and its
/// index answers the check a filled library makes on every open. A filled row leaves that range,
/// so the next page is the range's head again, with no cursor and no sort.
const UNFILLED_PAGE: &str =
    "SELECT id, artist, album, genre FROM tracks WHERE artist_key IS NULL LIMIT ?1";

const FILL: &str =
    "UPDATE tracks SET artist_key = ?1, album_key = ?2, genre_key = ?3 WHERE id = ?4";

/// One row's three keys, each field lowercased the way `UNICODE_LOWER` compares it and a missing
/// one read as `''`, so a `BINARY` order over a key is the collation's order over its field.
pub(crate) struct OrderKeys {
    pub artist: String,
    pub album: String,
    pub genre: String,
}

impl OrderKeys {
    pub(crate) fn of(artist: Option<&str>, album: Option<&str>, genre: Option<&str>) -> Self {
        Self { artist: lowered(artist), album: lowered(album), genre: lowered(genre) }
    }
}

fn lowered(field: Option<&str>) -> String {
    field.unwrap_or_default().to_lowercase()
}

/// The three columns a row's keys come from.
struct Unfilled {
    id: i64,
    artist: Option<String>,
    album: Option<String>,
    genre: Option<String>,
}

by_name!(Unfilled { id, artist, album, genre });

/// Fills the keys of every row written before they existed and returns how many rows it wrote.
/// Run before any list reads: an unfilled key sorts first.
///
/// One transaction, read a page at a time: the pages come in title-key order, scattered over the
/// table and three indexes, so a commit per page would pay a WAL checkpoint nearly every time.
/// Nothing else writes while the store opens, and an interrupted fill rolls back to be redone.
pub(crate) async fn fill_missing(db: &DbPool) -> Result<u64, AppError> {
    db.transaction(|tx| {
        let mut written = 0;
        loop {
            let filled = fill_page(tx)?;
            if filled == 0 {
                return Ok(written);
            }
            written += filled;
        }
    })
    .await
}

fn fill_page(tx: &Transaction<'_>) -> Result<u64, AppError> {
    let page: Vec<Unfilled> = sql::fetch_all(tx, UNFILLED_PAGE, (FILL_PAGE_ROWS,))?;
    let mut filled = 0;
    for row in &page {
        let keys = OrderKeys::of(row.artist.as_deref(), row.album.as_deref(), row.genre.as_deref());
        filled += sql::execute(tx, FILL, (keys.artist, keys.album, keys.genre, row.id))?;
    }
    Ok(filled)
}
