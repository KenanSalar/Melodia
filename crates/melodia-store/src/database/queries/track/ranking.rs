//! The play-count rankings: the Favorites hero and its Most Played tab, and the library-wide Most
//! Played tab.

use sqlx::{AssertSqlSafe, QueryBuilder, Sqlite};

use super::keyset::{push_needle, unpack_ids};
use crate::database::DbPool;
use melodia_core::entities::track;
use melodia_core::entities::track_list::{MostPlayedTotals, TrackListScope};
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
    let facts = super::scope_facts(db, TrackListScope::Favorites).await?;

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

    Ok(track::FavoriteStats {
        count: facts.tracks,
        total_duration_ms: facts.duration_ms,
        artists: facts.artists,
        albums: facts.albums,
        artwork_paths,
    })
}

/// Every track the library-wide Most Played tab ranks.
const PLAYED: &str = "play_count > 0";

/// Every track Favorites' Most Played tab ranks.
const PLAYED_FAVORITES: &str = "is_favorite = TRUE AND play_count > 0";

/// The library-wide Most Played tab's ids, ranked by [`MOST_PLAYED_ORDER`] and narrowed by
/// `needle`: what a grid holding only ids keeps.
pub async fn most_played_ids(db: &DbPool, needle: &Needle) -> Result<Vec<i64>, AppError> {
    ranked_ids(db, PLAYED, needle).await
}

/// [`most_played_ids`] over the favorites alone.
pub async fn most_played_favorite_ids(db: &DbPool, needle: &Needle) -> Result<Vec<i64>, AppError> {
    ranked_ids(db, PLAYED_FAVORITES, needle).await
}

/// Read back packed, as `track_ids` argues: a played set can be most of a library.
async fn ranked_ids(
    db: &DbPool,
    membership: &'static str,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT group_concat(id, ',') FROM (SELECT id FROM tracks WHERE ",
    );
    qb.push(membership);
    if !needle.is_empty() {
        qb.push(" AND ");
        push_needle(&mut qb, needle);
    }
    qb.push(" ORDER BY ").push(MOST_PLAYED_ORDER).push(")");
    let packed: Option<String> =
        qb.build_query_scalar().persistent(false).fetch_one(db.read()).await?;
    packed.map_or_else(|| Ok(Vec::new()), |packed| unpack_ids(&packed))
}

/// What the library-wide Most Played tab adds up to, whatever its filter shows: the band states
/// the page, not the filter.
pub async fn most_played_totals(db: &DbPool) -> Result<MostPlayedTotals, AppError> {
    ranked_totals(db, PLAYED).await
}

/// [`most_played_totals`] over the favorites alone.
pub async fn most_played_favorite_totals(db: &DbPool) -> Result<MostPlayedTotals, AppError> {
    ranked_totals(db, PLAYED_FAVORITES).await
}

async fn ranked_totals(
    db: &DbPool,
    membership: &'static str,
) -> Result<MostPlayedTotals, AppError> {
    let (tracks, duration_ms, plays): (i64, i64, i64) = sqlx::query_as(AssertSqlSafe(format!(
        "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0), COALESCE(SUM(play_count), 0) \
           FROM tracks WHERE {membership}"
    )))
    .fetch_one(db.read())
    .await?;
    Ok(MostPlayedTotals {
        tracks: i32::try_from(tracks).unwrap_or(i32::MAX),
        duration_ms,
        plays: i32::try_from(plays).unwrap_or(i32::MAX),
    })
}
