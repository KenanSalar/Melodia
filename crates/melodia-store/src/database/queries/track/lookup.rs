//! Narrow projections by id, id lookups by path or hash, and the work lists the one-shot sweeps
//! page through.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use rusqlite::Connection;

use super::file_rows::{FILE_ORDER, IN_LIBRARY, OUTSIDE_IDS, WHOLE_FILE};
use super::membership::Membership;
use super::{columns, json_array};
use crate::database::rows::FromRow;
use crate::database::sql::{self, Sql};
use crate::database::{DbPool, chunked_in_query, in_id_order, select_in_id_order};
use melodia_core::entities::cue::{CueTrack, SheetTrack};
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
    format!(
        "SELECT {} FROM tracks WHERE is_favorite = TRUE AND {IN_LIBRARY}",
        columns::SCROBBLE_ROW.joined()
    )
});

/// Every favorite the Favorites page shows, as scrobble rows, so connecting a service backfills its
/// loves without re-toggling each heart.
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

static LIBRARY_TRACK_COUNT: LazyLock<String> =
    LazyLock::new(|| format!("SELECT COUNT(*) FROM tracks WHERE {IN_LIBRARY}"));

/// How many tracks the library holds, for the diagnostics bundle and a filtered Songs list's total.
pub async fn count_tracks(db: &DbPool) -> Result<i64, AppError> {
    db.read(|conn| sql::fetch_one(conn, LIBRARY_TRACK_COUNT.as_str(), ())).await
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

/// [`missing_ids`] inside a transaction, for a write inserting the ids that still have a row. A file
/// opened from outside the library has one, though no library page shows it.
pub(crate) fn missing_ids_on(conn: &Connection, ids: &[i64]) -> Result<HashSet<i64>, AppError> {
    ids_answering(
        conn,
        ids,
        "SELECT j.value FROM json_each(?) j WHERE j.value NOT IN (SELECT id FROM tracks)",
    )
}

static FILED_OUTSIDE: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT j.value FROM json_each(?) j WHERE j.value IN ({OUTSIDE_IDS})")
});

/// The ids in `ids` filed outside the library, whose tags no album or join row holds.
pub async fn filed_outside(db: &DbPool, ids: &[i64]) -> Result<HashSet<i64>, AppError> {
    let ids = ids.to_vec();
    db.read(move |conn| ids_answering(conn, &ids, FILED_OUTSIDE.as_str())).await
}

/// Each of `ids` an external sheet carved out of its image, with that sheet.
pub async fn get_sheet_tracks_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<SheetTrack>, AppError> {
    let ids = ids.to_vec();
    let rows: Vec<(i64, String, CueTrack)> = db
        .read(move |conn| {
            chunked_in_query(conn, &ids, |placeholders| {
                format!(
                    "SELECT t.id, c.path, t.cue_track FROM tracks t \
                     JOIN cue_sheets c ON c.id = t.cue_sheet_id WHERE t.id IN ({placeholders})"
                )
            })
        })
        .await?;
    Ok(rows.into_iter().map(|(id, sheet, track)| SheetTrack { id, sheet, track }).collect())
}

/// The ids `statement` answers out of `ids`, which it reads from one `json_each` bind.
fn ids_answering(
    conn: &Connection,
    ids: &[i64],
    statement: &'static str,
) -> Result<HashSet<i64>, AppError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let answered: Vec<i64> = sql::fetch_all(conn, statement, (json_array(ids),))?;
    Ok(answered.into_iter().collect())
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

/// The rows of each path or hash asked for, by copy in path order, each copy's in its sheet's order:
/// one row for a whole file, one per track for an image a cue sheet splits. A path holds one copy,
/// a hash every file carrying it, split or not.
#[derive(Debug, Default)]
pub struct FileTracks(HashMap<String, Vec<FileCopy>>);

/// One file's rows under a [`FileTracks`] key.
#[derive(Debug)]
struct FileCopy {
    path: String,
    tracks: Vec<(Option<CueTrack>, i64)>,
}

impl FileCopy {
    fn whole(&self) -> Option<i64> {
        self.tracks.iter().find(|(cue_track, _)| cue_track.is_none()).map(|&(_, id)| id)
    }

    fn track(&self, number: CueTrack) -> Option<i64> {
        self.tracks.iter().find(|(cue_track, _)| *cue_track == Some(number)).map(|&(_, id)| id)
    }
}

impl FileTracks {
    /// Files each row under its key and its copy. `FILE_ORDER` sorts paths without case, so two
    /// copies named alike interleave by number, and a row joins its copy by the exact path.
    fn from_rows(rows: Vec<(String, String, Option<CueTrack>, i64)>) -> Self {
        let mut found = Self::default();
        for (key, path, cue_track, id) in rows {
            let copies = found.0.entry(key).or_default();
            match copies.iter_mut().find(|copy| copy.path == path) {
                Some(copy) => copy.tracks.push((cue_track, id)),
                None => copies.push(FileCopy { path, tracks: vec![(cue_track, id)] }),
            }
        }
        found
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// The rows of one copy under `key`, in play order, for a caller naming no track: a copy played
    /// whole where there is one, else every track of the lowest path's.
    pub fn ids(&self, key: &str) -> impl Iterator<Item = i64> + '_ {
        let copies = self.0.get(key).map_or(&[][..], Vec::as_slice);
        let copy = copies.iter().find(|copy| copy.whole().is_some()).or(copies.first());
        copy.into_iter().flat_map(|copy| copy.tracks.iter().map(|&(_, id)| id))
    }

    /// The row of the sheet's track `number` under `key`, from the lowest path's copy holding one.
    pub fn track(&self, key: &str, number: CueTrack) -> Option<i64> {
        self.0.get(key)?.iter().find_map(|copy| copy.track(number))
    }

    /// The row of the lowest path's copy played whole under `key`.
    pub fn whole(&self, key: &str) -> Option<i64> {
        self.0.get(key)?.iter().find_map(FileCopy::whole)
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

/// The rows of every file carrying each of `hashes`: any copy plays the same audio for a playlist
/// re-match, split by a cue sheet or not.
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
