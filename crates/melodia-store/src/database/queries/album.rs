use std::collections::HashMap;

use rusqlite::Transaction;

use crate::database::rows::by_name;
use crate::database::sql::{self, Sql};
use crate::database::{
    DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_query, chunked_in_statements, placeholders,
    select_in_id_order,
};
use melodia_core::entities::{album, tags};
use melodia_core::error::AppError;

struct TrackRelease {
    id: i64,
    tags: album::ReleaseTagRow,
}

by_name!(TrackRelease { id, tags: album::ReleaseTagRow });

/// The release tags behind each of `ids`, in the order given.
///
/// One row per *track*, not per album, so the Edit-Tags form folds them with `common_str` the way
/// it folds every other column and a selection spanning two releases shows the sentinel. A track
/// with no album yields the default row, which reads as "says nothing" — the same as a release
/// carrying none.
pub async fn get_release_tags_for_tracks(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<album::ReleaseTagRow>, AppError> {
    let wanted = ids.to_vec();
    let rows: Vec<TrackRelease> = db
        .read(move |conn| {
            chunked_in_query(conn, &wanted, |placeholders| {
                format!(
                    "SELECT t.id, \
                        COALESCE(al.label, '') AS label, \
                        COALESCE(al.catalog_number, '') AS catalog_number, \
                        COALESCE(al.barcode, '') AS barcode, \
                        COALESCE(al.media, '') AS media, \
                        COALESCE(al.release_type, '') AS release_type, \
                        COALESCE(al.release_country, '') AS release_country, \
                        COALESCE(al.is_compilation, FALSE) AS is_compilation \
                     FROM tracks t LEFT JOIN albums al ON al.id = t.album_id \
                     WHERE t.id IN ({placeholders})"
                )
            })
        })
        .await?;

    let by_id: HashMap<i64, album::ReleaseTagRow> =
        rows.into_iter().map(|row| (row.id, row.tags)).collect();
    Ok(ids.iter().map(|id| by_id.get(id).cloned().unwrap_or_default()).collect())
}

/// The Albums grid's cards for `ids`, in the order asked for.
pub async fn get_albums_by_ids(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<album::AlbumStats>, AppError> {
    db.read(move |conn| {
        select_in_id_order(conn, "SELECT * FROM album_stats", &ids, |a: &album::AlbumStats| a.id)
    })
    .await
}

pub async fn get_album_by_id(db: &DbPool, id: i64) -> Result<album::AlbumStats, AppError> {
    db.read(move |conn| sql::fetch_optional(conn, "SELECT * FROM album_stats WHERE id = ?", (id,)))
        .await?
        .ok_or_else(|| AppError::not_found("Album", id))
}

pub async fn get_albums_by_artist(
    db: &DbPool,
    artist_id: i64,
) -> Result<Vec<album::AlbumStats>, AppError> {
    db.read(move |conn| {
        sql::fetch_all(
            conn,
            "SELECT * FROM album_stats \
             WHERE id IN (SELECT album_id FROM album_artists WHERE artist_id = ?) \
             ORDER BY year ASC",
            (artist_id,),
        )
    })
    .await
}

/// Overwrites the albums' `artwork_path` inside the tag edit's transaction, where
/// `update_album_artwork_from_tracks` only fills empty ones.
pub(crate) fn set_album_artwork(
    tx: &Transaction<'_>,
    album_ids: &[i64],
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    let updates = chunked_in_statements(artwork_path, album_ids, |placeholders| {
        format!("UPDATE albums SET artwork_path = ? WHERE id IN ({placeholders})")
    });
    for update in updates {
        update.execute(tx)?;
    }
    Ok(())
}

/// Empties the release columns a tag edit cleared, on the albums it touched: `upsert_album`
/// coalesces a NULL away and ORs `is_compilation`, so it can never say "this release has no label".
///
/// A flag per column rather than assembled `SET` clauses, so the names stay literals and a `false`
/// flag leaves its column as it was.
pub(crate) fn clear_release_tags(
    tx: &Transaction<'_>,
    album_ids: &[i64],
    cleared: tags::ClearedReleaseTags,
) -> Result<(), AppError> {
    if album_ids.is_empty() || cleared.is_empty() {
        return Ok(());
    }
    // Reserve the seven flag binds.
    for chunk in album_ids.chunks(MAX_BINDS_PER_STATEMENT - 7) {
        let id_list = placeholders(chunk.len());
        let mut sql = Sql::vouched(format!(
            "UPDATE albums SET
                 label = CASE WHEN ? THEN NULL ELSE label END,
                 catalog_number = CASE WHEN ? THEN NULL ELSE catalog_number END,
                 barcode = CASE WHEN ? THEN NULL ELSE barcode END,
                 media = CASE WHEN ? THEN NULL ELSE media END,
                 release_type = CASE WHEN ? THEN NULL ELSE release_type END,
                 release_country = CASE WHEN ? THEN NULL ELSE release_country END,
                 is_compilation = CASE WHEN ? THEN FALSE ELSE is_compilation END
             WHERE id IN ({id_list})"
        ));
        sql.bind(cleared.label)
            .bind(cleared.catalog_number)
            .bind(cleared.barcode)
            .bind(cleared.media)
            .bind(cleared.release_type)
            .bind(cleared.release_country)
            .bind(cleared.compilation);
        for &id in chunk {
            sql.bind(id);
        }
        sql.execute(tx)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/album_tests.rs"]
mod tests;
