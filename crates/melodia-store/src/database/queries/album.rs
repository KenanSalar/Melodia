use sqlx::AssertSqlSafe;

use crate::database::{DbPool, select_in_id_order};
use melodia_core::entities::{album, tags};
use melodia_core::error::AppError;

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
    // A flat tuple rather than `(i64, ReleaseTagRow)`: sqlx decodes a tuple element per *column*,
    // so a nested row type there asks it to decode a struct out of one value.
    type Row = (i64, String, String, String, String, String, String, bool);
    let rows: Vec<Row> = crate::database::chunked_in_query(db.read(), ids, |placeholders| {
        format!(
            "SELECT t.id, \
                COALESCE(al.label, ''), \
                COALESCE(al.catalog_number, ''), \
                COALESCE(al.barcode, ''), \
                COALESCE(al.media, ''), \
                COALESCE(al.release_type, ''), \
                COALESCE(al.release_country, ''), \
                COALESCE(al.is_compilation, FALSE) \
             FROM tracks t LEFT JOIN albums al ON al.id = t.album_id \
             WHERE t.id IN ({placeholders})"
        )
    })
    .await?;

    let by_id: std::collections::HashMap<i64, album::ReleaseTagRow> = rows
        .into_iter()
        .map(
            |(
                id,
                label,
                catalog_number,
                barcode,
                media,
                release_type,
                release_country,
                compilation,
            )| {
                (
                    id,
                    album::ReleaseTagRow {
                        label,
                        catalog_number,
                        barcode,
                        media,
                        release_type,
                        release_country,
                        is_compilation: compilation,
                    },
                )
            },
        )
        .collect();
    Ok(ids.iter().map(|id| by_id.get(id).cloned().unwrap_or_default()).collect())
}

/// The Albums grid's cards for `ids`, in the order asked for.
pub async fn get_albums_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<album::AlbumStats>, AppError> {
    select_in_id_order(db, "SELECT * FROM album_stats", ids, |a: &album::AlbumStats| a.id).await
}

pub async fn get_album_by_id(db: &DbPool, id: i64) -> Result<album::AlbumStats, AppError> {
    sqlx::query_as::<_, album::AlbumStats>("SELECT * FROM album_stats WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Album", id))
}

pub async fn get_albums_by_artist(
    db: &DbPool,
    artist_id: i64,
) -> Result<Vec<album::AlbumStats>, AppError> {
    let albums = sqlx::query_as::<_, album::AlbumStats>(
        "SELECT * FROM album_stats \
         WHERE id IN (SELECT album_id FROM album_artists WHERE artist_id = ?) \
         ORDER BY year ASC",
    )
    .bind(artist_id)
    .fetch_all(db.read())
    .await?;
    Ok(albums)
}

/// Overwrites the albums' `artwork_path` inside the tag edit's transaction, where
/// `update_album_artwork_from_tracks` only fills empty ones.
pub(crate) async fn set_album_artwork(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    album_ids: &[i64],
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    for chunk in album_ids.chunks(crate::database::IDS_PER_SET) {
        crate::database::set_where_id_in("UPDATE albums SET artwork_path = ?", artwork_path, chunk)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Empties the release columns a tag edit cleared, on the albums it touched: `upsert_album`
/// coalesces a NULL away and ORs `is_compilation`, so it can never say "this release has no label".
///
/// A flag per column rather than assembled `SET` clauses, so the names stay literals and a `false`
/// flag leaves its column as it was.
pub(crate) async fn clear_release_tags(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    album_ids: &[i64],
    cleared: tags::ClearedReleaseTags,
) -> Result<(), AppError> {
    if album_ids.is_empty() || cleared.is_empty() {
        return Ok(());
    }
    // Reserve the seven flag binds.
    for chunk in album_ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 7) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!(
            "UPDATE albums SET
                 label = CASE WHEN ? THEN NULL ELSE label END,
                 catalog_number = CASE WHEN ? THEN NULL ELSE catalog_number END,
                 barcode = CASE WHEN ? THEN NULL ELSE barcode END,
                 media = CASE WHEN ? THEN NULL ELSE media END,
                 release_type = CASE WHEN ? THEN NULL ELSE release_type END,
                 release_country = CASE WHEN ? THEN NULL ELSE release_country END,
                 is_compilation = CASE WHEN ? THEN FALSE ELSE is_compilation END
             WHERE id IN ({placeholders})"
        );
        let mut query = sqlx::query(AssertSqlSafe(sql))
            .persistent(false)
            .bind(cleared.label)
            .bind(cleared.catalog_number)
            .bind(cleared.barcode)
            .bind(cleared.media)
            .bind(cleared.release_type)
            .bind(cleared.release_country)
            .bind(cleared.compilation);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(&mut **tx).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/album_tests.rs"]
mod tests;
