//! The play-count rankings: the Favorites hero and its Most Played tab, and the library-wide Most
//! Played tab.

use sqlx::{AssertSqlSafe, QueryBuilder, Sqlite};

use super::keyset::{fetch_packed_ids, push_needle};
use crate::database::DbPool;
use melodia_core::entities::track;
use melodia_core::entities::track_list::{MostPlayedTotals, TrackListScope};
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;

/// The `ORDER BY` body every "most played" surface ranks with, the Favorites hero included.
///
/// The keys after `play_count DESC` make it *total*, so the grid holds still between refreshes and
/// the mosaic, a second query over the same rows, picks the same covers: the most recently played
/// first, then the newest added, then the id. RFC 3339 `last_played` sorts as text, NULL last.
const MOST_PLAYED_ORDER: &str = "play_count DESC, last_played DESC, date_added DESC, id ASC";

/// Aggregate stats for the Favorites hero header: count, total duration, the artist and album
/// spread, up to 4 artwork paths.
pub async fn get_favorite_stats(db: &DbPool) -> Result<track::FavoriteStats, AppError> {
    let facts = super::scope_facts(db, TrackListScope::Favorites).await?;

    // The collage is the head of the Most Played tab, each cover at its best slot, rather than a
    // second ranking to keep in step. Ranked over every favourite so unplayed ones fill it once
    // the played covers run out, and never padded with repeats: `compose_cover` lays out one to
    // four.
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
    fetch_packed_ids(db, qb).await
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
