//! The play-count rankings: the Favorites hero and its Most Played tab, and the library-wide Most
//! Played tab.

use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use melodia_core::entities::track;
use melodia_core::error::AppError;

/// The `ORDER BY` body every "most played" surface ranks with — the Favorites grid tab and the
/// hero mosaic that has to agree with it.
///
/// `play_count DESC` is the ranking; the three keys after it are what make it *total*. Without
/// them `SQLite` may hand back tied rows in any order, so the grid could re-order between
/// refreshes and the mosaic — a second query over the same rows — had no reason to pick the same
/// four covers. `last_played DESC` breaks a tie toward the most recently played (RFC-3339 text
/// whose lexical order is chronological; NULLs sort last under `DESC`). `date_added DESC` is
/// reachable wherever that ties, keeping the mosaic's fill newest-first, and `id ASC` closes the
/// last gap.
const MOST_PLAYED_ORDER: &str = "play_count DESC, last_played DESC, date_added DESC, id ASC";

/// Aggregate stats for the Favorites hero header: count, total duration, up to 4 artwork paths.
pub async fn get_favorite_stats(db: &DbPool) -> Result<track::FavoriteStats, AppError> {
    let (count, total_duration_ms): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks WHERE is_favorite = TRUE",
    )
    .fetch_one(db.read())
    .await?;

    // Up to 4 *distinct* artworks for the Favorites banner collage: walk the
    // favourites in `MOST_PLAYED_ORDER` and keep each cover's best slot, which
    // makes the collage literally the head of the Most Played tab rather than a
    // second ranking that has to be kept in step with it.
    //
    // The rank spans *all* favourites, not just `play_count > 0` as the grid
    // does, so once the played covers run out the remainder fills the collage —
    // unplayed rows sort last under `play_count DESC`. Duplicates are
    // deliberately not synthesised: `compose_cover` lays 1–4 sources out per
    // count, so a one-album-heavy library gets one full-bleed cover rather than
    // the same artwork four times.
    let artwork_paths: Vec<String> = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT artwork_path FROM ( \
            SELECT artwork_path, \
                   ROW_NUMBER() OVER (ORDER BY {MOST_PLAYED_ORDER}) AS pos \
              FROM tracks \
             WHERE is_favorite = TRUE AND artwork_path IS NOT NULL AND artwork_path <> '' \
         ) \
         GROUP BY artwork_path \
         ORDER BY MIN(pos) \
         LIMIT 4"
    )))
    .fetch_all(db.read())
    .await?;

    Ok(track::FavoriteStats { count, total_duration_ms, artwork_paths })
}

/// Favorite tracks by play count, most played first (only those with `play_count` > 0), ordered
/// by [`MOST_PLAYED_ORDER`].
///
/// Returns the whole set rather than a top N — the Most Played tab is a
/// virtualized grid, so it has nothing to gain by truncating. Once `ANALYZE`
/// has run (shutdown's `PRAGMA optimize`) the partial `idx_tracks_play_count`
/// drives the scan and supplies the leading term, leaving the three
/// tiebreakers to a sort inside each tied group; with no stats yet the planner
/// prefers the `is_favorite` equality index and sorts the lot.
pub async fn get_most_played_favorites(
    db: &DbPool,
) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    // The order's trailing keys are read, not selected — `MostPlayedFavorite`
    // is the card projection and has no slot for a timestamp.
    let cols = track::most_played_columns();
    let rows = sqlx::query_as::<_, track::MostPlayedFavorite>(AssertSqlSafe(format!(
        "SELECT {cols} \
           FROM tracks \
          WHERE is_favorite = TRUE AND play_count > 0 \
          ORDER BY {MOST_PLAYED_ORDER}"
    )))
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// Tracks by play count across the whole library, most played first (only those with
/// `play_count` > 0). Sibling of [`get_most_played_favorites`] without the `is_favorite` filter —
/// drives the Recently-Played view's "Most Played" tab through the same `MostPlayedFavorite` card
/// projection and [`MOST_PLAYED_ORDER`].
///
/// **Returns the whole set rather than a top N**, because the tab that reads it is a virtualized
/// grid and a cap there is a ceiling the user can scroll into with nothing saying why the list
/// stops. This predicate reaches everything ever played, so on a large library each
/// `stats_changed` tick — one per finished track, while the page is on screen — materializes a row
/// per played track, and the Rust side holds them until the section is left
/// (`RecentlyPlayedUiState::most_played`). Partial index `idx_tracks_play_count` covers the `WHERE`
/// and the leading `ORDER BY` term.
pub async fn get_most_played(db: &DbPool) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    let cols = track::most_played_columns();
    let rows = sqlx::query_as::<_, track::MostPlayedFavorite>(AssertSqlSafe(format!(
        "SELECT {cols} \
           FROM tracks \
          WHERE play_count > 0 \
          ORDER BY {MOST_PLAYED_ORDER}"
    )))
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}
