//! What a hero states about a scope's tracks beyond its own stats row, and the covers they carry,
//! answered in SQL over the same membership its keyset lists, so a page holding only ids still has
//! its facts.

use rusqlite::Connection;

use super::membership::Membership;
use crate::database::DbPool;
use crate::database::queries::smart_playlist;
use crate::database::sql::Sql;
use melodia_core::entities::track_list::{ScopeFacts, TrackListScope};
use melodia_core::error::AppError;

/// How many tracks `scope` holds, how long they run and how many artists and albums they span.
///
/// The spread counts `tracks.artist_id` and `tracks.album_id`, each track's first credit and its
/// album. `COUNT(DISTINCT …)` skips a NULL, so a track with no album adds none, where an untagged
/// artist is the one unknown-artist row the scan credits it to. A smart playlist is counted under
/// its cap.
pub async fn scope_facts(db: &DbPool, scope: TrackListScope) -> Result<ScopeFacts, AppError> {
    db.read(move |conn| facts_on(conn, scope)).await
}

/// [`scope_facts`] on a connection the caller holds.
pub(super) fn facts_on(conn: &Connection, scope: TrackListScope) -> Result<ScopeFacts, AppError> {
    let membership = Membership::resolve(conn, scope)?;
    let mut sql = Sql::new(
        "SELECT COUNT(*), COALESCE(SUM(t.duration_ms), 0), \
                COUNT(DISTINCT t.artist_id), COUNT(DISTINCT t.album_id) FROM ",
    );
    membership.push_members(&mut sql);
    let (tracks, duration_ms, artists, albums): (i64, i64, i64, i64) = sql.fetch_one(conn)?;
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
    db.read(move |conn| {
        let membership = Membership::resolve(conn, scope)?;
        let mut sql = Sql::new("SELECT t.genre, COUNT(*) FROM ");
        membership.push_members(&mut sql);
        membership.push_and(&mut sql);
        sql.push("t.genre IS NOT NULL GROUP BY t.genre ORDER BY COUNT(*) DESC, MIN(t.id)");
        sql.fetch_all(conn)
    })
    .await
}

/// Up to `limit` distinct covers `scope`'s tracks carry, in the order the scope first reaches
/// each, so a smart playlist offers the covers of what its rules match and its cap keeps.
///
/// Ordered by `MIN(position)` rather than by the position of whichever row the grouping kept, or a
/// cover an early and a late track share could sort last. A scope with no order of its own falls
/// back to the id.
pub async fn cover_paths(
    db: &DbPool,
    scope: TrackListScope,
    limit: i64,
) -> Result<Vec<String>, AppError> {
    db.read(move |conn| {
        let membership = Membership::resolve(conn, scope)?;
        cover_paths_of(conn, &membership, limit)
    })
    .await
}

fn cover_paths_of(
    conn: &Connection,
    membership: &Membership,
    limit: i64,
) -> Result<Vec<String>, AppError> {
    let mut sql = Sql::new("SELECT t.artwork_path FROM ");
    membership.push_source(&mut sql);
    membership.push_and(&mut sql);
    sql.push(
        "t.artwork_path IS NOT NULL AND t.artwork_path != '' GROUP BY t.artwork_path ORDER BY MIN(",
    )
    .push(membership.position().unwrap_or("t.id"))
    .push(") LIMIT ")
    .push_bind(limit);
    sql.fetch_all(conn)
}

/// The cover `scope`'s tracks reach first, [`cover_paths`]'s first answer. An uncapped smart
/// playlist walks its natural order instead of numbering every match to find the lowest.
pub async fn first_cover(db: &DbPool, scope: TrackListScope) -> Result<Option<String>, AppError> {
    db.read(move |conn| {
        let membership = Membership::resolve(conn, scope)?;
        if let Membership::SmartPlaylist(criteria) = &membership
            && criteria.limit.is_none()
        {
            return smart_playlist::first_cover(conn, criteria);
        }
        Ok(cover_paths_of(conn, &membership, 1)?.into_iter().next())
    })
    .await
}

#[cfg(test)]
#[path = "../tests/scope_facts_tests.rs"]
mod tests;
