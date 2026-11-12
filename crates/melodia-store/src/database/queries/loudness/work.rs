//! What a pass reads: the albums whose values went stale, and the work left, a keyset page at a
//! time so no read holds the library.
//!
//! **An album is measured as one whole only while every library member is untagged and hashed.**
//! One with a tagged member measures its untagged members alone, re-measuring a tagged file being
//! out of scope, and one with an unhashed member waits for `retroactive_hash` to reach it.

use std::sync::LazyLock;

use rusqlite::Connection;

use super::{AlbumJob, MATCHING_MEASUREMENT, MeasureTrack, untagged};
use crate::database::queries::track::file_rows::IN_LIBRARY;
use crate::database::queries::track::{detail_order, json_array};
use crate::database::rows::by_name;
use crate::database::{DbPool, sql};
use melodia_core::entities::track_list::TrackSortField;
use melodia_core::error::AppError;

/// One row per library album, its id first so a read of ids takes the tally whole: its members, and
/// how many of them carry a gain tag, have no hash yet, or lack a matching measurement holding this
/// album's values (`short`), a row holding another album's included. `narrow` is a term on
/// `t.album_id` inside the grouping, where the album index answers it.
fn album_tally(narrow: &str) -> String {
    format!(
        "SELECT t.album_id, COUNT(*) AS members,
                SUM(NOT ({untagged})) AS tagged,
                SUM(t.file_hash IS NULL) AS unhashed,
                SUM(l.album_id IS NOT t.album_id) AS short
         FROM tracks t {MATCHING_MEASUREMENT}
         WHERE t.album_id IS NOT NULL {narrow} AND {IN_LIBRARY}
         GROUP BY t.album_id",
        untagged = untagged!(t),
    )
}

/// An album still owed a measurement as one whole, as the tally's `HAVING`: in the grouping
/// itself, a page walks the album index in order and stops at its limit, where a filter around it
/// sorts the whole tally first.
const ALBUM_OWED: &str = "HAVING tagged = 0 AND unhashed = 0 AND short > 0";

/// A library track owed a measurement of its own: on no album, or on one a tagged member keeps
/// from being measured whole.
static TRACK_OWED: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{untagged_t} AND t.file_hash IS NOT NULL AND l.track_id IS NULL AND {IN_LIBRARY}
         AND (t.album_id IS NULL OR t.album_id IN
              (SELECT m.album_id FROM tracks m WHERE m.album_id IS NOT NULL AND NOT ({untagged_m})))",
        untagged_t = untagged!(t),
        untagged_m = untagged!(m),
    )
});

/// Albums whose stored values no longer describe them: gone, a different size, holding a tagged,
/// unhashed or unmatched member, or a member carrying another album's values. The outer aliases
/// stay off `l`, which the tally's join would shadow.
static STALE_ALBUMS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT held.album_id FROM
             (SELECT album_id, MIN(album_tracks) AS least, MAX(album_tracks) AS most
              FROM track_loudness WHERE album_id IS NOT NULL GROUP BY album_id) held
         LEFT JOIN ({tally}) tally ON tally.album_id = held.album_id
         WHERE tally.album_id IS NULL OR held.least <> tally.members OR held.most <> tally.members
            OR tally.tagged > 0 OR tally.unhashed > 0 OR tally.short > 0",
        tally = album_tally(""),
    )
});

/// Sends every album whose stored values went stale back to the work list, answering how many.
/// Reads first, so the common wake where nothing moved never takes the writer.
pub async fn settle(db: &DbPool) -> Result<usize, AppError> {
    let stale: Vec<i64> = db.read(|conn| sql::fetch_all(conn, STALE_ALBUMS.as_str(), ())).await?;
    if stale.is_empty() {
        return Ok(0);
    }
    let albums = json_array(&stale);
    db.write(move |conn| {
        sql::execute(
            conn,
            "UPDATE track_loudness
             SET album_id = NULL, album_tracks = NULL, album_gain = NULL, album_peak = NULL
             WHERE album_id IN (SELECT value FROM json_each(?1))",
            (albums,),
        )
    })
    .await?;
    Ok(stale.len())
}

/// The work a pass has left, an album counting every member it will decode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pending {
    pub albums: u32,
    pub tracks: u32,
}

static PENDING: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT COUNT(*), COALESCE(SUM(members), 0)
                  + (SELECT COUNT(*) FROM tracks t {MATCHING_MEASUREMENT} WHERE {owed})
         FROM ({tally} {ALBUM_OWED})",
        tally = album_tally(""),
        owed = TRACK_OWED.as_str(),
    )
});

pub async fn pending(db: &DbPool) -> Result<Pending, AppError> {
    let (albums, tracks) = db.read(|conn| sql::fetch_one(conn, PENDING.as_str(), ())).await?;
    Ok(Pending { albums, tracks })
}

static OWED_ALBUMS_AFTER: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{tally} {ALBUM_OWED} ORDER BY t.album_id LIMIT ?2",
        tally = album_tally("AND t.album_id > ?1"),
    )
});

/// Up to `limit` albums owed a measurement, after `after_album` in id order, with their members.
pub async fn album_jobs_after(
    db: &DbPool,
    after_album: i64,
    limit: i64,
) -> Result<Vec<AlbumJob>, AppError> {
    db.read(move |conn| {
        let albums = sql::fetch_all(conn, OWED_ALBUMS_AFTER.as_str(), (after_album, limit))?;
        album_jobs(conn, &albums)
    })
    .await
}

static OWED_TRACKS_AFTER: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT t.id, t.file_hash, t.file_path AS path, t.cue_start, t.cue_end
         FROM tracks t {MATCHING_MEASUREMENT}
         WHERE t.id > ?1 AND {owed} ORDER BY t.id LIMIT ?2",
        owed = TRACK_OWED.as_str(),
    )
});

/// Up to `limit` tracks owed a measurement of their own, after `after_id` in id order.
pub async fn track_jobs_after(
    db: &DbPool,
    after_id: i64,
    limit: i64,
) -> Result<Vec<MeasureTrack>, AppError> {
    db.read(move |conn| sql::fetch_all(conn, OWED_TRACKS_AFTER.as_str(), (after_id, limit))).await
}

/// The work among a handful of tracks: the albums owed whole that hold any of them, and those
/// owed alone.
#[derive(Debug, Clone, Default)]
pub struct QueuedJobs {
    pub albums: Vec<AlbumJob>,
    pub tracks: Vec<MeasureTrack>,
}

static QUEUED_ALBUMS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{tally} {ALBUM_OWED} ORDER BY t.album_id",
        tally = album_tally(
            "AND t.album_id IN (SELECT q.album_id FROM tracks q \
             WHERE q.id IN (SELECT value FROM json_each(?1)))"
        ),
    )
});

static QUEUED_TRACKS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT t.id, t.file_hash, t.file_path AS path, t.cue_start, t.cue_end
         FROM tracks t {MATCHING_MEASUREMENT}
         WHERE t.id IN (SELECT value FROM json_each(?1)) AND {owed} ORDER BY t.id",
        owed = TRACK_OWED.as_str(),
    )
});

/// The work `ids` touch, for a pass to take ahead of its keyset.
pub async fn queued_jobs(db: &DbPool, ids: &[i64]) -> Result<QueuedJobs, AppError> {
    if ids.is_empty() {
        return Ok(QueuedJobs::default());
    }
    let ids = json_array(ids);
    db.read(move |conn| {
        let albums = sql::fetch_all(conn, QUEUED_ALBUMS.as_str(), (ids.as_str(),))?;
        Ok(QueuedJobs {
            albums: album_jobs(conn, &albums)?,
            tracks: sql::fetch_all(conn, QUEUED_TRACKS.as_str(), (ids.as_str(),))?,
        })
    })
    .await
}

/// A member of the albums one read asked for, grouped into jobs after.
struct AlbumMember {
    album_id: i64,
    track: MeasureTrack,
}

by_name!(AlbumMember { album_id, track: MeasureTrack });

/// Members in the order Album Detail lists them, so the album meter hears the album played through.
static ALBUM_MEMBERS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT t.album_id, t.id, t.file_hash, t.file_path AS path, t.cue_start, t.cue_end
         FROM tracks t
         WHERE t.album_id IN (SELECT value FROM json_each(?1)) AND {IN_LIBRARY}
         ORDER BY t.album_id, {order}",
        order = detail_order(TrackSortField::TrackNumber),
    )
});

/// `albums` with their members, read on the connection that found them so both see one snapshot.
fn album_jobs(conn: &Connection, albums: &[i64]) -> Result<Vec<AlbumJob>, AppError> {
    if albums.is_empty() {
        return Ok(Vec::new());
    }
    let members: Vec<AlbumMember> =
        sql::fetch_all(conn, ALBUM_MEMBERS.as_str(), (json_array(albums),))?;
    let mut jobs: Vec<AlbumJob> = Vec::with_capacity(albums.len());
    for AlbumMember { album_id, track } in members {
        match jobs.last_mut() {
            Some(job) if job.album_id == album_id => job.members.push(track),
            _ => jobs.push(AlbumJob { album_id, members: vec![track] }),
        }
    }
    Ok(jobs)
}
