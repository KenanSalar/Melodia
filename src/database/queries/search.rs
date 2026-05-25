use crate::database::DbPool;
use crate::entities::{album, artist, track};
use crate::error::AppError;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResults {
    pub tracks: Vec<track::TrackListRow>,
    pub albums: Vec<album::AlbumStats>,
    pub artists: Vec<artist::ArtistStats>,
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
        });
    }

    let fts_query = build_fts_query(trimmed);
    let cols = track::track_list_columns_prefixed("t");

    // Albums and artists use LIKE (small tables, no FTS needed)
    let pattern = like_pattern_escape(trimmed);

    // Run all three queries concurrently — the read pool supports concurrent readers.
    let sql = format!(
        "SELECT {cols} FROM tracks t
         JOIN tracks_fts fts ON fts.rowid = t.id
         WHERE tracks_fts MATCH ?
         ORDER BY rank LIMIT 50"
    );
    let tracks_fut = sqlx::query_as::<_, track::TrackListRow>(&sql)
        .bind(&fts_query)
        .fetch_all(db.read());

    let albums_fut = sqlx::query_as::<_, album::AlbumStats>(
        "SELECT * FROM album_stats
         WHERE name LIKE ? ESCAPE '\\' OR artist_name LIKE ? ESCAPE '\\'
         ORDER BY name ASC LIMIT 20"
    )
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(db.read());

    let artists_fut = sqlx::query_as::<_, artist::ArtistStats>(
        "SELECT * FROM artist_stats WHERE name LIKE ? ESCAPE '\\' ORDER BY name ASC LIMIT 20"
    )
    .bind(&pattern)
    .fetch_all(db.read());

    let (tracks, albums, artists) = tokio::try_join!(tracks_fut, albums_fut, artists_fut)?;

    Ok(SearchResults {
        tracks,
        albums,
        artists,
    })
}

#[cfg(test)]
#[path = "tests/search_tests.rs"]
mod tests;
