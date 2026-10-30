//! Writes to track rows: counters, the per-row flags, artwork and hashes.

use sqlx::AssertSqlSafe;

use crate::database::{DbPool, IDS_PER_SET, chunked_in_query_on, set_where_id_in};
use melodia_core::error::AppError;

pub async fn update_play_count(db: &DbPool, id: i64) -> Result<(), AppError> {
    let now = melodia_core::utils::now_rfc3339();
    sqlx::query("UPDATE tracks SET play_count = play_count + 1, last_played = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

pub async fn update_skip_count(db: &DbPool, id: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE tracks SET skip_count = skip_count + 1 WHERE id = ?")
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// [`update_play_count`] over many tracks at once: each `(track id, plays)` adds its plays, and
/// every one of them is stamped `last_played = now`.
pub async fn add_play_counts(
    db: &DbPool,
    increments: &[(i64, u32)],
    now: &str,
) -> Result<(), AppError> {
    add_counts(db, "play_count", increments, Some(now)).await
}

/// [`update_skip_count`] over many tracks at once.
pub async fn add_skip_counts(db: &DbPool, increments: &[(i64, u32)]) -> Result<(), AppError> {
    add_counts(db, "skip_count", increments, None).await
}

/// One `UPDATE` per chunk adding each row's increment to `column`, plus the `last_played` stamp
/// when there is one.
async fn add_counts(
    db: &DbPool,
    column: &'static str,
    increments: &[(i64, u32)],
    last_played: Option<&str>,
) -> Result<(), AppError> {
    // The stamp is the one bind that is not part of a row.
    let max_rows = (crate::database::MAX_BINDS_PER_STATEMENT - usize::from(last_played.is_some()))
        / crate::database::CASE_BINDS_PER_ROW;
    let stamp = if last_played.is_some() { ", last_played = ?" } else { "" };

    for chunk in increments.chunks(max_rows) {
        let sql = format!(
            "UPDATE tracks SET {column} = {column} + {}{stamp} WHERE id IN ({})",
            crate::database::case_by_id(chunk.len()),
            crate::database::placeholders(chunk.len()),
        );

        let mut query = sqlx::query(AssertSqlSafe(sql));
        for &(id, n) in chunk {
            query = query.bind(id).bind(i64::from(n));
        }
        if let Some(now) = last_played {
            query = query.bind(now);
        }
        for &(id, _) in chunk {
            query = query.bind(id);
        }
        query.persistent(false).execute(db.write()).await?;
    }
    Ok(())
}

pub async fn update_last_position(db: &DbPool, id: i64, position_ms: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE tracks SET last_position = ? WHERE id = ?")
        .bind(position_ms)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

pub async fn set_favorite(db: &DbPool, ids: &[i64], favorite: bool) -> Result<(), AppError> {
    for chunk in ids.chunks(IDS_PER_SET) {
        set_where_id_in("UPDATE tracks SET is_favorite = ?", favorite, chunk)
            .execute(db.write())
            .await?;
    }
    Ok(())
}

/// Clears the heart on those of `ids` that carry one, answering exactly those, so an undo puts
/// back only what this took. One transaction: a failed chunk leaves nothing cleared that the
/// answer doesn't name.
pub async fn unfavorite(db: &DbPool, ids: &[i64]) -> Result<Vec<i64>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = db.write().begin().await?;
    let cleared: Vec<(i64,)> = chunked_in_query_on(&mut tx, ids, |placeholders| {
        format!(
            "UPDATE tracks SET is_favorite = 0 WHERE is_favorite = 1 AND id IN ({placeholders})
             RETURNING id"
        )
    })
    .await?;
    tx.commit().await?;
    Ok(cleared.into_iter().map(|(id,)| id).collect())
}

/// Sets the star `rating`, which `library::ratings::set_rating` has already clamped.
pub async fn set_rating(db: &DbPool, ids: &[i64], rating: i32) -> Result<(), AppError> {
    for chunk in ids.chunks(IDS_PER_SET) {
        set_where_id_in("UPDATE tracks SET rating = ?", rating, chunk).execute(db.write()).await?;
    }
    Ok(())
}

/// Overwrites `artwork_path` inside the tag edit's transaction, so `None` nulls it as
/// `update_track_metadata`'s `COALESCE` can't.
pub(crate) async fn set_track_artwork(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    for chunk in ids.chunks(IDS_PER_SET) {
        set_where_id_in("UPDATE tracks SET artwork_path = ?", artwork_path, chunk)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Blanks every `date_modified`, answering how many rows moved. NULL honestly means unknown: it
/// matches no file's mtime in `track_is_current`, and the scan refills it from the `stat` that
/// proved the file exists.
pub async fn mark_every_track_stale(db: &DbPool) -> Result<u64, AppError> {
    let result =
        sqlx::query("UPDATE tracks SET date_modified = NULL WHERE date_modified IS NOT NULL")
            .execute(db.write())
            .await?;
    Ok(result.rows_affected())
}

/// Writes `(id, file_hash, date_modified)` triples, a CTE-driven UPDATE per chunk inside one
/// transaction, so a backfill over a large library pays one commit.
pub async fn batch_update_hashes(
    db: &DbPool,
    updates: &[(i64, String, Option<String>)],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 3;
    const CHUNK_SIZE: usize = crate::database::MAX_BINDS_PER_STATEMENT / COLS_PER_ROW;

    if updates.is_empty() {
        return Ok(());
    }

    let mut tx = db.write().begin().await?;
    for chunk in updates.chunks(CHUNK_SIZE) {
        let placeholders =
            std::iter::repeat_n("(?,?,?)", chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "WITH v(id, h, m) AS (VALUES {placeholders})
             UPDATE tracks
                SET file_hash = (SELECT h FROM v WHERE v.id = tracks.id),
                    date_modified = (SELECT m FROM v WHERE v.id = tracks.id)
              WHERE id IN (SELECT id FROM v)"
        );
        let mut q = sqlx::query(AssertSqlSafe(sql)).persistent(false);
        for (id, hash, mtime) in chunk {
            q = q.bind(id).bind(hash).bind(mtime);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
