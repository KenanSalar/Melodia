//! The play-count rankings: the Favorites hero and its Most Played tab, and the library-wide Most
//! Played tab.

use sqlx::{AssertSqlSafe, QueryBuilder, Sqlite};

use super::columns;
use super::keyset::push_needle;
use crate::database::DbPool;
use melodia_core::entities::track;
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;

/// The `ORDER BY` body every "most played" surface ranks with, the Favorites hero mosaic included.
///
/// `play_count DESC` is the ranking; the three keys after it are what make it *total*. Without
/// them `SQLite` may hand back tied rows in any order, so the grid could re-order between
/// refreshes and the mosaic — a second query over the same rows — had no reason to pick the same
/// four covers. `last_played DESC` breaks a tie toward the most recently played (RFC-3339 text
/// whose lexical order is chronological; NULLs sort last under `DESC`). `date_added DESC` is
/// reachable wherever that ties, keeping the mosaic's fill newest-first, and `id ASC` closes the
/// last gap.
const MOST_PLAYED_ORDER: &str = "play_count DESC, last_played DESC, date_added DESC, id ASC";

/// Aggregate stats for the Favorites hero header: count, total duration, the artist and album
/// spread, up to 4 artwork paths.
pub async fn get_favorite_stats(db: &DbPool) -> Result<track::FavoriteStats, AppError> {
    // `COUNT(DISTINCT …)` skips NULLs, so an untagged track adds no artist or album, as
    // `ui::hero_folds::fold_tracks` reads the same two columns off a list's rows.
    let (count, total_duration_ms, artists, albums): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0), \
                COUNT(DISTINCT artist_id), COUNT(DISTINCT album_id) \
           FROM tracks WHERE is_favorite = TRUE",
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

    Ok(track::FavoriteStats { count, total_duration_ms, artists, albums, artwork_paths })
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
    let cols = columns::MOST_PLAYED.joined();
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
    let cols = columns::MOST_PLAYED.joined();
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

/// The library-wide Most Played tab as `(id, play_count)` pairs, ranked as [`get_most_played`]
/// ranks its cards and narrowed by `needle`: what a grid holding only ids keeps.
pub async fn most_played_ids(db: &DbPool, needle: &Needle) -> Result<Vec<(i64, i32)>, AppError> {
    ranked_ids(db, "play_count > 0", needle).await
}

/// [`most_played_ids`] over the favorites alone, as [`get_most_played_favorites`] ranks them.
pub async fn most_played_favorite_ids(
    db: &DbPool,
    needle: &Needle,
) -> Result<Vec<(i64, i32)>, AppError> {
    ranked_ids(db, "is_favorite = TRUE AND play_count > 0", needle).await
}

async fn ranked_ids(
    db: &DbPool,
    membership: &'static str,
    needle: &Needle,
) -> Result<Vec<(i64, i32)>, AppError> {
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT id, play_count FROM tracks WHERE ");
    qb.push(membership);
    if !needle.is_empty() {
        qb.push(" AND ");
        push_needle(&mut qb, needle);
    }
    qb.push(" ORDER BY ").push(MOST_PLAYED_ORDER);
    let ranked = qb.build_query_as::<(i64, i32)>().persistent(false).fetch_all(db.read()).await?;
    Ok(ranked)
}
