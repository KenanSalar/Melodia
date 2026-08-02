use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use crate::entities::{album, artist, genre, track};
use crate::error::AppError;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResults {
    pub tracks: Vec<track::TrackListRow>,
    pub albums: Vec<album::AlbumStats>,
    pub artists: Vec<artist::ArtistStats>,
    pub genres: Vec<genre::GenreStats>,
}

/// Build an FTS5 MATCH expression from a raw query string.
/// Each whitespace-separated word is quoted and suffixed with `*` for prefix matching.
/// Returns an empty string if the input is empty/whitespace-only.
pub(crate) fn build_fts_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Pre-size for the common case (no embedded `"`): each word adds `""*`
    // around its content plus a separating space. `len() * 2` covers the
    // rare double-up from `fts5_escape` without a realloc.
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

/// Wrap `s` in `%…%` and escape `\`, `%`, `_` for a `LIKE … ESCAPE '\'`
/// pattern in a single pass into a pre-allocated `String`. Replaces three
/// chained `.replace()` calls (3 passes, 3 allocs) with one.
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
    let cols = track::track_list_columns_prefixed("t");

    // Name matching for albums / artists / genres is LIKE against their
    // `*_stats` views — small tables, no index to gain. One pattern serves
    // every name `?` below; the FTS expression is reused for the
    // track-derived arms.
    let pattern = like_pattern_escape(trimmed);

    // Run all four queries concurrently — the read pool supports concurrent readers.
    let sql = format!(
        "SELECT {cols} FROM tracks t
         JOIN tracks_fts fts ON fts.rowid = t.id
         WHERE tracks_fts MATCH ?
         ORDER BY rank LIMIT 50"
    );
    let tracks_fut = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .bind(&fts_query)
        .fetch_all(db.read());

    // Both strips also match through *their own tracks*, reusing the same
    // FTS expression the Songs list runs. Without it a query that only
    // reaches track metadata — a song title, a year, a composer, a genre —
    // left both strips empty and, because the Top Result ranks over these
    // two lists, left the page with no Top Result card at all beside a
    // full list of songs.
    //
    // Deriving it from the FTS match rather than from `genre_id` is what
    // keeps the two halves of the page agreeing about what "matched":
    // whatever the index covers, the strips cover. It also subsumes the
    // genre-only lookup this replaces, since `genre` is one of the indexed
    // columns.
    //
    // The name arms are ordered first because all three lists share 20
    // slots: a broad query can turn up more track-derived rows than that,
    // and without the sort an album named for the query could be pushed
    // out of its own search. Tiers 1-6 of the Top Result scan the whole
    // list for a name match, so only the fall-through tiers and the
    // strips' display order depend on this.
    //
    // Both `IS NOT NULL` guards are insurance, not corrections: under
    // `OR … IN` a NULL from the subquery yields NULL, which drops the row
    // exactly as FALSE does. They earn their place the moment either arm
    // is spelled `NOT IN`, where one NULL swallows every row instead.
    let albums_fut = sqlx::query_as::<_, album::AlbumStats>(
        "SELECT * FROM album_stats
         WHERE name LIKE ? ESCAPE '\\' OR artist_name LIKE ? ESCAPE '\\'
            OR id IN (SELECT t.album_id FROM tracks t
                      JOIN tracks_fts f ON f.rowid = t.id
                      WHERE tracks_fts MATCH ? AND t.album_id IS NOT NULL)
         ORDER BY (name LIKE ? ESCAPE '\\' OR artist_name LIKE ? ESCAPE '\\') DESC,
                  name ASC
         LIMIT 20"
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
            OR id IN (SELECT t.artist_id FROM tracks t
                      JOIN tracks_fts f ON f.rowid = t.id
                      WHERE tracks_fts MATCH ? AND t.artist_id IS NOT NULL)
         ORDER BY (name LIKE ? ESCAPE '\\') DESC, name ASC
         LIMIT 20"
    )
    .bind(&pattern)
    .bind(&fts_query)
    .bind(&pattern)
    .fetch_all(db.read());

    let genres_fut = sqlx::query_as::<_, genre::GenreStats>(
        "SELECT * FROM genre_stats WHERE name LIKE ? ESCAPE '\\' ORDER BY name ASC LIMIT 20"
    )
    .bind(&pattern)
    .fetch_all(db.read());

    let (tracks, albums, artists, genres) =
        tokio::try_join!(tracks_fut, albums_fut, artists_fut, genres_fut)?;

    Ok(SearchResults {
        tracks,
        albums,
        artists,
        genres,
    })
}

#[cfg(test)]
#[path = "tests/search_tests.rs"]
mod tests;
