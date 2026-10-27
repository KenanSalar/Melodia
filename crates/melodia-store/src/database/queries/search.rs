//! Search: one FTS5 match over `tracks_fts`, plus a name lookup per strip, run together.
//!
//! **The diacritic folding stops at the index.** `tracks_fts` tokenizes under
//! `remove_diacritics 2`, so an unaccented query reaches an accented *track*, but the `name LIKE`
//! arms aren't folded: an accented album or artist is reached only through its tracks, in a Top
//! Result fall-through tier, and an accented genre, whose only arm is that `LIKE`, never surfaces.
//!
//! The two FTS subqueries are unbounded: fts5 scores every match either way, and each costs a
//! `tracks` rowid lookup per match, two more walks rather than a doubling of one.

use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use crate::database::queries::track::columns;
use melodia_core::entities::search::SearchResults;
use melodia_core::entities::{album, artist, genre, track};
use melodia_core::error::AppError;

/// An FTS5 `MATCH` expression from a raw query, each word quoted and `*`-suffixed for a prefix
/// match; empty for a blank query.
pub(crate) fn build_fts_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut fts_query = String::with_capacity(trimmed.len() * 2 + 4);
    for (i, word) in trimmed.split_whitespace().enumerate() {
        if i > 0 {
            fts_query.push(' ');
        }
        fts_query.push('"');
        for ch in word.chars() {
            fts_query.push(ch);
            if ch == '"' {
                fts_query.push('"');
            }
        }
        fts_query.push_str("\"*");
    }
    fts_query
}

/// `s` wrapped in `%…%`, with `\`, `%` and `_` escaped for `LIKE … ESCAPE '\'`.
fn like_pattern_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    out.push('%');
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

pub async fn search_all(db: &DbPool, query: &str) -> Result<SearchResults, AppError> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(SearchResults {
            tracks: vec![],
            albums: vec![],
            artists: vec![],
            genres: vec![],
        });
    }

    let fts_query = build_fts_query(trimmed);
    let cols = columns::TRACK_LIST.prefixed("t");

    // `LIKE` over the `*_stats` views: small tables, no index to gain.
    let pattern = like_pattern_escape(trimmed);

    // `tracks_fts MATCH`, never the alias: fts5 names the match column after the table, so
    // `fts MATCH` fails to prepare. `rank` is bm25 under the weights the newest `tracks_fts`
    // rebuild pins, which pick the 50 rows that survive the `LIMIT`, not only their order.
    let sql = format!(
        "SELECT {cols} FROM tracks t
         JOIN tracks_fts fts ON fts.rowid = t.id
         WHERE tracks_fts MATCH ?
         ORDER BY rank LIMIT 50"
    );
    let tracks_fut = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .bind(&fts_query)
        .fetch_all(db.read());

    // Both strips also match through their own tracks, so a query reaching only track metadata
    // still fills them and the Top Result ranked over them: whatever the index covers, the
    // strips cover. The name arms sort first so an album named for the query isn't pushed out of
    // its 20 slots by track-derived rows. The `IS NOT NULL` guards cost nothing under `IN`, and
    // spelled `NOT IN` one NULL would swallow every row.
    let albums_fut = sqlx::query_as::<_, album::AlbumStats>(
        "SELECT * FROM album_stats
         WHERE name LIKE ? ESCAPE '\\' OR artist_name LIKE ? ESCAPE '\\'
            OR id IN (SELECT t.album_id FROM tracks t
                      JOIN tracks_fts f ON f.rowid = t.id
                      WHERE tracks_fts MATCH ? AND t.album_id IS NOT NULL)
         ORDER BY (name LIKE ? ESCAPE '\\' OR artist_name LIKE ? ESCAPE '\\') DESC,
                  name ASC
         LIMIT 20",
    )
    .bind(&pattern)
    .bind(&pattern)
    .bind(&fts_query)
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(db.read());

    let artists_fut = sqlx::query_as::<_, artist::ArtistStats>(
        "SELECT * FROM artist_stats
         WHERE name LIKE ? ESCAPE '\\'
            OR id IN (SELECT ta.artist_id FROM track_artists ta
                      JOIN tracks_fts f ON f.rowid = ta.track_id
                      WHERE tracks_fts MATCH ?)
         ORDER BY (name LIKE ? ESCAPE '\\') DESC, name ASC
         LIMIT 20",
    )
    .bind(&pattern)
    .bind(&fts_query)
    .bind(&pattern)
    .fetch_all(db.read());

    let genres_fut = sqlx::query_as::<_, genre::GenreStats>(
        "SELECT * FROM genre_stats WHERE name LIKE ? ESCAPE '\\' ORDER BY name ASC LIMIT 20",
    )
    .bind(&pattern)
    .fetch_all(db.read());

    let (tracks, albums, artists, genres) =
        tokio::try_join!(tracks_fut, albums_fut, artists_fut, genres_fut)?;

    Ok(SearchResults { tracks, albums, artists, genres })
}

#[cfg(test)]
#[path = "tests/search_tests.rs"]
mod tests;
