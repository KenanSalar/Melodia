use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use melodia_core::entities::album;
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

pub async fn get_all_albums(db: &DbPool) -> Result<Vec<album::AlbumStats>, AppError> {
    let albums =
        sqlx::query_as::<_, album::AlbumStats>("SELECT * FROM album_stats ORDER BY name ASC")
            .fetch_all(db.read())
            .await?;
    Ok(albums)
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

/// Authoritatively set `artwork_path` for one or more albums by ID, within a
/// caller-supplied transaction. Unlike `update_album_artwork_from_tracks`
/// (which only backfills rows `WHERE artwork_path IS NULL OR = ''`), this
/// overwrites an existing cover — the tag-edit orchestrator needs to replace
/// an album's art when the user edits embedded artwork. Tx-scoped so it lands
/// in the same transaction as the per-track metadata refresh.
pub async fn set_album_artwork(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    album_ids: &[i64],
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    if album_ids.is_empty() {
        return Ok(());
    }
    // Reserve 1 bind slot for the `artwork_path` parameter itself.
    for chunk in album_ids.chunks(crate::database::SQLITE_BIND_LIMIT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("UPDATE albums SET artwork_path = ? WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(artwork_path);
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
