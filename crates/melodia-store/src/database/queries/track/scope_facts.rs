//! What a hero states about a scope's tracks beyond its own stats row, answered in SQL over the
//! same membership its keyset lists, so a page holding only ids still has its facts.

use sqlx::{QueryBuilder, Sqlite};

use super::membership::Membership;
use crate::database::DbPool;
use melodia_core::entities::track_list::{ScopeFacts, TrackListScope};
use melodia_core::error::AppError;

/// How many tracks `scope` holds, how long they run and how many artists and albums they span.
///
/// The spread counts `tracks.artist_id` and `tracks.album_id`, each track's first credit and its
/// album. `COUNT(DISTINCT …)` skips a NULL, so a track with no album adds none, where an untagged
/// artist is the one unknown-artist row the scan credits it to. A smart playlist is counted under
/// its cap.
pub async fn scope_facts(db: &DbPool, scope: TrackListScope) -> Result<ScopeFacts, AppError> {
    let membership = Membership::resolve(db, scope).await?;
    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(*), COALESCE(SUM(t.duration_ms), 0), \
                COUNT(DISTINCT t.artist_id), COUNT(DISTINCT t.album_id) FROM ",
    );
    membership.push_source(&mut qb);
    let (tracks, duration_ms, artists, albums): (i64, i64, i64, i64) =
        qb.build_query_as().persistent(false).fetch_one(db.read()).await?;
    Ok(ScopeFacts { tracks, duration_ms, artists, albums })
}

/// Every rendered genre line `scope` carries, with how many of its tracks carry it, the commonest
/// first. An untagged track adds no line.
///
/// Lines rather than genres, so a caller tallying names reads what each track's row shows: a line
/// holds every tag its track has, in tag order, and the line its order is read off for a tie is
/// the one most of the scope shares.
pub async fn genre_lines(
    db: &DbPool,
    scope: TrackListScope,
) -> Result<Vec<(String, i64)>, AppError> {
    let membership = Membership::resolve(db, scope).await?;
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT t.genre, COUNT(*) FROM ");
    membership.push_source(&mut qb);
    membership.push_and(&mut qb);
    qb.push("t.genre IS NOT NULL GROUP BY t.genre ORDER BY COUNT(*) DESC, MIN(t.id)");
    let lines = qb.build_query_as().persistent(false).fetch_all(db.read()).await?;
    Ok(lines)
}
