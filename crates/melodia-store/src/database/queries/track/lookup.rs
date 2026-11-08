//! Narrow projections by id, id lookups by path or hash, and the work lists the one-shot sweeps
//! page through.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use rusqlite::Connection;

use super::membership::Membership;
use super::{columns, json_array};
use crate::database::rows::FromRow;
use crate::database::sql::{self, Sql};
use crate::database::{DbPool, chunked_in_query, select_in_id_order};
use melodia_core::entities::track;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::AppError;

static TRACK_META_BY_ID: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT {} FROM tracks WHERE id = ? LIMIT 1", columns::TRACK_META.joined())
});

/// The Now Playing view's technical chips, `None` for a missing id.
pub async fn get_track_meta(db: &DbPool, id: i64) -> Result<Option<track::TrackMeta>, AppError> {
    db.read(move |conn| sql::fetch_optional(conn, TRACK_META_BY_ID.as_str(), (id,))).await
}

static SCROBBLE_ROW_BY_ID: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT {} FROM tracks WHERE id = ? LIMIT 1", columns::SCROBBLE_ROW.joined())
});

/// The columns a scrobble needs, for the detector's per-track enrichment; `None` for a missing id.
pub async fn get_scrobble_row(
    db: &DbPool,
    id: i64,
) -> Result<Option<track::ScrobbleRow>, AppError> {
    db.read(move |conn| sql::fetch_optional(conn, SCROBBLE_ROW_BY_ID.as_str(), (id,))).await
}

static FAVORITE_SCROBBLE_ROWS: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT {} FROM tracks WHERE is_favorite = TRUE", columns::SCROBBLE_ROW.joined())
});

/// Every favorite's scrobble row, so connecting a service backfills its loves without re-toggling
/// each heart.
pub async fn get_favorite_scrobble_rows(db: &DbPool) -> Result<Vec<track::ScrobbleRow>, AppError> {
    db.read(|conn| sql::fetch_all(conn, FAVORITE_SCROBBLE_ROWS.as_str(), ())).await
}

/// [`get_scrobble_row`] for a multi-selection's favorite-to-love sync, in query order, the order of
/// loves not mattering.
pub async fn get_scrobble_rows_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::ScrobbleRow>, AppError> {
    let cols = columns::SCROBBLE_ROW.joined();
    let ids = ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &ids, |placeholders| {
            format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})")
        })
    })
    .await
}

pub async fn get_track_file_path(db: &DbPool, id: i64) -> Result<Option<String>, AppError> {
    db.read(move |conn| {
        sql::fetch_optional(conn, "SELECT file_path FROM tracks WHERE id = ? LIMIT 1", (id,))
    })
    .await
}

/// How many tracks the library holds, for the diagnostics bundle and a filtered Songs list's total.
pub async fn count_tracks(db: &DbPool) -> Result<i64, AppError> {
    db.read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM tracks", ())).await
}

/// `TrackSummary` rows for `ids`, in order: what the queue, Now Playing and playback read.
pub async fn get_track_summaries_by_ids(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<track::TrackSummary>, AppError> {
    rows_in_id_order(db, columns::TRACK_SUMMARY.joined(), ids, |t: &track::TrackSummary| t.id).await
}

/// `TrackListRow` rows for `ids`, in order: the page a list holding only its ids draws.
pub async fn get_track_list_rows_by_ids(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<track::TrackListRow>, AppError> {
    rows_in_id_order(db, columns::TRACK_LIST.joined(), ids, |t: &track::TrackListRow| t.id).await
}

/// Most Played cards for `ids`, in order.
pub async fn get_most_played_by_ids(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    rows_in_id_order(db, columns::MOST_PLAYED.joined(), ids, |t: &track::MostPlayedFavorite| t.id)
        .await
}

/// `cols` of every track in `ids`, in the order `ids` asked for.
async fn rows_in_id_order<T>(
    db: &DbPool,
    cols: &'static str,
    ids: Vec<i64>,
    id_of: impl Fn(&T) -> i64 + Send + 'static,
) -> Result<Vec<T>, AppError>
where
    T: FromRow + Send + 'static,
{
    db.read(move |conn| {
        select_in_id_order(conn, &format!("SELECT {cols} FROM tracks"), &ids, id_of)
    })
    .await
}

/// `TagEditRow`s for `ids`, in order, for the Edit Track Information dialog: the editable
/// single-valued columns and the Summary tab's technical ones.
///
/// **No multi-valued field**: the dialog reads artists, genres and role credits as rows through
/// `get_track_{credits,role_credits,genres}_by_ids`. An editor filled from the rendered
/// `tracks.genre` splits `Chanson, Francaise` into two genres, and saving makes that permanent.
pub async fn get_tag_edit_rows_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::TagEditRow>, AppError> {
    let cols = columns::TRACK_TAG_EDIT.joined();
    rows_in_id_order(db, cols, ids.to_vec(), |t: &track::TagEditRow| t.id).await
}

/// `(id, file_path)` for `ids`, in order, so the tag writer's self-write marks land
/// deterministically.
pub async fn get_track_paths_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<(i64, String)>, AppError> {
    rows_in_id_order(db, "id, file_path", ids.to_vec(), |&(id, _): &(i64, String)| id).await
}

/// `TrackLinks` for `ids`, in order: the foreign keys the now-playing bar's menu needs, which
/// `TrackSummary` doesn't carry.
pub async fn get_track_links_by_ids(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<track::TrackLinks>, AppError> {
    rows_in_id_order(db, columns::TRACK_LINKS.joined(), ids, |l: &track::TrackLinks| l.id).await
}

/// The ids in `ids` that no longer have a row. Asked this way round because the answer is almost
/// always none: asking which survive hands a queue the size of the library back a row per track.
pub async fn missing_ids(db: &DbPool, ids: &[i64]) -> Result<HashSet<i64>, AppError> {
    let ids = ids.to_vec();
    db.read(move |conn| missing_ids_on(conn, &ids)).await
}

/// [`missing_ids`] inside a transaction, for a write inserting the ids that still have a row.
pub(crate) fn missing_ids_on(conn: &Connection, ids: &[i64]) -> Result<HashSet<i64>, AppError> {
    ids_outside(conn, ids, &Membership::Library)
}

/// The ids in `ids` that `scope` no longer holds, asked the way round [`missing_ids`] is.
pub async fn missing_from_scope(
    db: &DbPool,
    scope: TrackListScope,
    ids: &[i64],
) -> Result<HashSet<i64>, AppError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let ids = ids.to_vec();
    db.read(move |conn| {
        let membership = Membership::resolve(conn, scope)?;
        ids_outside(conn, &ids, &membership)
    })
    .await
}

fn ids_outside(
    conn: &Connection,
    ids: &[i64],
    membership: &Membership,
) -> Result<HashSet<i64>, AppError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let mut sql = Sql::new("SELECT j.value FROM json_each(");
    sql.push_bind(json_array(ids)).push(") j WHERE j.value NOT IN (SELECT t.id FROM ");
    membership.push_members(&mut sql);
    sql.push(")");
    Ok(sql.fetch_all::<i64>(conn)?.into_iter().collect())
}

/// `file_path → id` for each of `paths` that has a row.
pub async fn get_track_ids_by_paths(
    db: &DbPool,
    paths: &[String],
) -> Result<HashMap<String, i64>, AppError> {
    let paths = paths.to_vec();
    let rows: Vec<(i64, String)> = db
        .read(move |conn| {
            chunked_in_query(conn, &paths, |placeholders| {
                format!("SELECT id, file_path FROM tracks WHERE file_path IN ({placeholders})")
            })
        })
        .await?;

    let mut map = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|(id, path)| (path, id)));
    Ok(map)
}

/// `file_hash → id` for each of `hashes` that has a row. Where duplicates share a hash any one wins,
/// either playing the same audio for a playlist re-match.
pub async fn get_track_ids_by_hashes(
    db: &DbPool,
    hashes: Vec<String>,
) -> Result<HashMap<String, i64>, AppError> {
    let rows: Vec<(i64, String)> = db
        .read(move |conn| {
            chunked_in_query(conn, &hashes, |placeholders| {
                format!(
                    "SELECT id, file_hash FROM tracks \
                         WHERE file_hash IS NOT NULL AND file_hash IN ({placeholders})"
                )
            })
        })
        .await?;

    let mut map = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|(id, hash)| (hash, id)));
    Ok(map)
}

/// `file_hash → [(id, file_path)]` of the rows carrying each of `hashes`, lowest id first: the ones
/// a moved file may re-point rather than duplicate. Hashes with no row are absent.
///
/// On a connection rather than the pool so the scan's ingest can ask inside its transaction and
/// the watcher's batch off the read pool, ahead of its own.
pub(crate) fn rows_by_hash_on(
    conn: &Connection,
    hashes: &[&str],
) -> Result<HashMap<String, Vec<(i64, String)>>, AppError> {
    let unique: HashSet<&str> = hashes.iter().copied().collect();
    let unique: Vec<&str> = unique.into_iter().collect();

    let rows: Vec<(String, i64, String)> = chunked_in_query(conn, &unique, |placeholders| {
        format!(
            "SELECT file_hash, id, file_path FROM tracks
             WHERE file_hash IN ({placeholders})
             ORDER BY id ASC"
        )
    })?;
    let mut by_hash: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (hash, id, path) in rows {
        by_hash.entry(hash).or_default().push((id, path));
    }
    Ok(by_hash)
}

/// The tracks with no `file_hash` yet, for the retroactive hash.
pub async fn get_unhashed_track_paths(db: &DbPool) -> Result<Vec<(i64, String)>, AppError> {
    db.read(|conn| {
        sql::fetch_all(conn, "SELECT id, file_path FROM tracks WHERE file_hash IS NULL", ())
    })
    .await
}

/// One page of unrated tracks past `after_id`, the one-shot rating import's work list.
///
/// `rating = 0` can't tell an untouched row from a cleared one, so the import runs once and
/// records that in `settings.json`. Keyset-paged because a first run selects the whole library,
/// and because the import rates as it goes, so rows leave the predicate and an `OFFSET` would skip
/// their neighbours.
pub async fn get_unrated_track_paths_after(
    db: &DbPool,
    after_id: i64,
    limit: i64,
) -> Result<Vec<(i64, String)>, AppError> {
    db.read(move |conn| {
        sql::fetch_all(
            conn,
            "SELECT id, file_path FROM tracks WHERE rating = 0 AND id > ? ORDER BY id LIMIT ?",
            (after_id, limit),
        )
    })
    .await
}
