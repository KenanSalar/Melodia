//! The track ids behind a set of entities, for the card grids' right-click actions, batched so
//! queueing five albums is one statement.
//!
//! **Each returns `(entity_id, track_id)` pairs, per-entity order preserved, grouping left to the
//! caller.** `chunked_in_query` splits on the entity ids, so each entity's rows share a chunk and
//! its `ORDER BY`; the entities' own order is the caller's to restore.

use std::sync::LazyLock;

use super::keyset::detail_order;
use crate::database::collation::UNICODE_LOWER;
use crate::database::queries::DISK_IGNORES_CASE;
use crate::database::{DbPool, chunked_in_query, sql};
use melodia_core::entities::track_list::TrackSortField;
use melodia_core::error::AppError;

/// Album tracks in Album Detail's default order, `ui::track_sort`'s `"track_number"` arm rather
/// than the bare columns, which sort a NULL first where the comparator puts an untagged track
/// last. `idx_tracks_album_disc_track` still answers the lookup, if no longer the order.
pub async fn track_ids_by_albums(
    db: &DbPool,
    album_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    let order = detail_order(TrackSortField::TrackNumber);
    let album_ids = album_ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &album_ids, |placeholders| {
            format!(
                "SELECT t.album_id, t.id FROM tracks t \
                 WHERE t.album_id IN ({placeholders}) \
                 ORDER BY {order}"
            )
        })
    })
    .await
}

/// Artist tracks in Artist Detail's default, `ui::track_sort`'s `"album"` arm. **Not `sort_key`**,
/// which queues by title under a page showing the catalogue by album.
///
/// **Joins where the page's keyset doesn't**, to project the artist id, so a row repeats across
/// selected artists and inside one crediting the same artist twice; the caller dedupes.
pub async fn track_ids_by_artists(
    db: &DbPool,
    artist_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    let order = detail_order(TrackSortField::Album);
    let artist_ids = artist_ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &artist_ids, |placeholders| {
            format!(
                "SELECT ta.artist_id, t.id FROM tracks t \
                 JOIN track_artists ta ON ta.track_id = t.id \
                 WHERE ta.artist_id IN ({placeholders}) \
                 ORDER BY {order}"
            )
        })
    })
    .await
}

/// Genre tracks in Genre Detail's own order, joined and deduped for the reason the artist arm
/// above gives, and ordered for the reason it gives too, both pages defaulting to `"album"`.
pub async fn track_ids_by_genres(
    db: &DbPool,
    genre_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    let order = detail_order(TrackSortField::Album);
    let genre_ids = genre_ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &genre_ids, |placeholders| {
            format!(
                "SELECT tg.genre_id, t.id FROM tracks t \
                 JOIN track_genres tg ON tg.track_id = t.id \
                 WHERE tg.genre_id IN ({placeholders}) \
                 ORDER BY {order}"
            )
        })
    })
    .await
}

/// Manual-playlist membership in playlist order. **Smart playlists have no `playlist_items` rows**
/// and come back empty here; resolving them means their stored criteria, which is why the caller
/// splits the two before asking.
pub async fn track_ids_by_playlists(
    db: &DbPool,
    playlist_ids: &[i64],
) -> Result<Vec<(i64, i64)>, AppError> {
    let playlist_ids = playlist_ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &playlist_ids, |placeholders| {
            format!(
                "SELECT playlist_id, track_id FROM playlist_items \
                 WHERE playlist_id IN ({placeholders}) \
                 ORDER BY position ASC"
            )
        })
    })
    .await
}

const IDS_IN_SUBTREE: &str = "SELECT id FROM tracks WHERE file_path >= ?1 AND file_path < ?2 \
     ORDER BY file_path COLLATE NOCASE ASC";

static IDS_IN_SUBTREE_ANY_CASE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT id FROM tracks \
         WHERE file_path COLLATE {UNICODE_LOWER} >= ?1 AND file_path COLLATE {UNICODE_LOWER} < ?2 \
         ORDER BY file_path COLLATE NOCASE ASC"
    )
});

/// Every track under `dir_path`, subdirectories included, in path order: what "play this folder"
/// means for a card, where [`super::get_tracks_in_directory`] is the one level Browse lists. An
/// artist folder of album folders would otherwise play nothing.
///
/// `COLLATE NOCASE` because Browse lists through `to_lowercase()`, and BINARY would play
/// `Zebra.flac` before `apple.flac`. ASCII against a Unicode fold, and a subtree against one level,
/// so **case is the whole of the agreement**.
///
/// An empty range retries ignoring case where the disk does, non-ASCII letters included: a folder
/// renamed by case alone keeps its rows under the old spelling until a scan, the watcher passing
/// only audio paths.
pub async fn track_ids_under_directory(db: &DbPool, dir_path: &str) -> Result<Vec<i64>, AppError> {
    let (from, before) = super::list::subtree_bounds(dir_path);
    db.read(move |conn| {
        let ids = sql::fetch_all(conn, IDS_IN_SUBTREE, (&from, &before))?;
        if ids.is_empty() && DISK_IGNORES_CASE {
            return sql::fetch_all(conn, IDS_IN_SUBTREE_ANY_CASE.as_str(), (from, before));
        }
        Ok(ids)
    })
    .await
}

#[cfg(test)]
#[path = "../tests/entity_ids_tests.rs"]
mod tests;
