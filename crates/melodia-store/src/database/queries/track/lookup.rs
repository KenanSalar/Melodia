//! Narrow projections by id, id lookups by path or hash, and the work lists the one-shot sweeps
//! page through.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use rusqlite::Connection;

use super::file_rows::{FILE_ORDER, WHOLE_FILE};
use super::membership::Membership;
use super::{columns, json_array};
use crate::database::rows::FromRow;
use crate::database::sql::{self, Sql};
use crate::database::{DbPool, chunked_in_query, in_id_order, select_in_id_order};
use melodia_core::entities::cue::CueTrack;
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

/// `(id, file_path)` for those of `ids` that play a whole file, in order, so the tag writer's
/// self-write marks land deterministically. A cue sheet's track owns no file: its image's tags
/// describe the whole disc, and writing one track's edit there would stamp it on every track.
pub async fn get_whole_file_paths_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<(i64, String)>, AppError> {
    let ids = ids.to_vec();
    db.read(move |conn| {
        let rows = chunked_in_query(conn, &ids, |placeholders| {
            format!(
                "SELECT id, file_path FROM tracks WHERE id IN ({placeholders}) AND {WHOLE_FILE}"
            )
        })?;
        Ok(in_id_order(rows, &ids, |&(id, _): &(i64, String)| id))
    })
    .await
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

/// The rows of each path or hash asked for, a file's in its sheet's order: one for a whole file,
/// one per track for an image a cue sheet splits. A caller matches on the sheet's number, or takes
/// them all.
#[derive(Debug, Default)]
pub struct FileTracks(HashMap<String, Vec<(Option<CueTrack>, i64)>>);

impl FileTracks {
    /// Files them under `key`, keeping only the first file each key meets: two copies of one
    /// image share a hash, and their tracks would otherwise interleave.
    fn from_rows(rows: Vec<(String, String, Option<CueTrack>, i64)>) -> Self {
        let mut first_path: HashMap<String, String> = HashMap::new();
        let mut tracks = Self::default();
        for (key, path, cue_track, id) in rows {
            if *first_path.entry(key.clone()).or_insert_with(|| path.clone()) == path {
                tracks.0.entry(key).or_default().push((cue_track, id));
            }
        }
        tracks
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Every row under `key`, in play order.
    pub fn ids(&self, key: &str) -> impl Iterator<Item = i64> + '_ {
        self.0.get(key).into_iter().flatten().map(|&(_, id)| id)
    }

    /// The row of the sheet's track `number` under `key`.
    pub fn track(&self, key: &str, number: CueTrack) -> Option<i64> {
        let tracks = self.0.get(key)?;
        tracks.iter().find(|(cue_track, _)| *cue_track == Some(number)).map(|&(_, id)| id)
    }
}

/// The rows of each of `paths` that has any.
pub async fn get_track_ids_by_paths(db: &DbPool, paths: &[String]) -> Result<FileTracks, AppError> {
    let paths = paths.to_vec();
    let rows = db
        .read(move |conn| {
            chunked_in_query(conn, &paths, |placeholders| {
                format!(
                    "SELECT file_path, file_path, cue_track, id FROM tracks
                     WHERE file_path IN ({placeholders})
                     ORDER BY {FILE_ORDER}"
                )
            })
        })
        .await?;
    Ok(FileTracks::from_rows(rows))
}

/// The rows of one file carrying each of `hashes`, the lowest path's: any copy plays the same
/// audio for a playlist re-match.
pub async fn get_track_ids_by_hashes(
    db: &DbPool,
    hashes: Vec<String>,
) -> Result<FileTracks, AppError> {
    let rows = db
        .read(move |conn| {
            chunked_in_query(conn, &hashes, |placeholders| {
                format!(
                    "SELECT file_hash, file_path, cue_track, id FROM tracks
                     WHERE file_hash IS NOT NULL AND file_hash IN ({placeholders})
                     ORDER BY {FILE_ORDER}"
                )
            })
        })
        .await?;
    Ok(FileTracks::from_rows(rows))
}

/// `file_hash → [(id, file_path)]` of the whole-file rows carrying each of `hashes`, lowest id
/// first: the ones a moved file may re-point rather than duplicate. Hashes with no row are absent.
///
/// On a connection rather than the pool so the scan's ingest can ask inside its transaction and
/// the watcher's batch off the read pool, ahead of its own.
pub(crate) fn rows_by_hash_on(
    conn: &Connection,
    hashes: &[&str],
) -> Result<HashMap<String, Vec<(i64, String)>>, AppError> {
    fetch_by_hash(conn, hashes, |placeholders| {
        format!(
            "SELECT file_hash, id, file_path FROM tracks
             WHERE file_hash IN ({placeholders}) AND {WHOLE_FILE}
             ORDER BY id ASC"
        )
    })
}

/// [`rows_by_hash_on`] for the files a cue sheet splits, one entry per file under its lowest row
/// id: a moved image takes all of its rows or none.
pub(crate) fn images_by_hash_on(
    conn: &Connection,
    hashes: &[&str],
) -> Result<HashMap<String, Vec<(i64, String)>>, AppError> {
    fetch_by_hash(conn, hashes, |placeholders| {
        format!(
            "SELECT file_hash, MIN(id), file_path FROM tracks
             WHERE file_hash IN ({placeholders}) AND cue_track IS NOT NULL
             GROUP BY file_hash, file_path
             ORDER BY MIN(id) ASC"
        )
    })
}

fn fetch_by_hash(
    conn: &Connection,
    hashes: &[&str],
    build_sql: impl Fn(&str) -> String,
) -> Result<HashMap<String, Vec<(i64, String)>>, AppError> {
    let unique: HashSet<&str> = hashes.iter().copied().collect();
    let unique: Vec<&str> = unique.into_iter().collect();

    let rows: Vec<(String, i64, String)> = chunked_in_query(conn, &unique, build_sql)?;
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

static UNRATED_WHOLE_FILES_AFTER: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT id, file_path FROM tracks
         WHERE rating = 0 AND id > ? AND {WHOLE_FILE}
         ORDER BY id LIMIT ?"
    )
});

/// One page of unrated tracks past `after_id`, the one-shot rating import's work list.
///
/// `rating = 0` can't tell an untouched row from a cleared one, so the import runs once and
/// records that in `settings.json`. Keyset-paged because a first run selects the whole library,
/// and because the import rates as it goes, so rows leave the predicate and an `OFFSET` would skip
/// their neighbours. A cue sheet's tracks are left out, their image's rating being the disc's.
pub async fn get_unrated_track_paths_after(
    db: &DbPool,
    after_id: i64,
    limit: i64,
) -> Result<Vec<(i64, String)>, AppError> {
    db.read(move |conn| sql::fetch_all(conn, UNRATED_WHOLE_FILES_AFTER.as_str(), (after_id, limit)))
        .await
}
