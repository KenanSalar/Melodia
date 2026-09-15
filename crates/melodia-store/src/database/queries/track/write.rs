//! Writes to track rows: counters, the per-row flags, artwork and hashes.

use sqlx::AssertSqlSafe;

use crate::database::DbPool;
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

/// Set `is_favorite` for one or more tracks by ID.
pub async fn set_favorite(db: &DbPool, ids: &[i64], favorite: bool) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    // Reserve 1 bind slot for the `favorite` parameter itself.
    for chunk in ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("UPDATE tracks SET is_favorite = ? WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(favorite);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(db.write()).await?;
    }
    Ok(())
}

/// Set the star `rating` (0–5) for one or more tracks by ID. Mirrors [`set_favorite`]: the value
/// is clamped by `library::ratings::set_rating`, chunked to stay inside one statement's bind
/// budget, and run non-persistently since the placeholder count is dynamic.
pub async fn set_rating(db: &DbPool, ids: &[i64], rating: i32) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    // Reserve 1 bind slot for the `rating` parameter itself.
    for chunk in ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("UPDATE tracks SET rating = ? WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(rating);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(db.write()).await?;
    }
    Ok(())
}

/// Authoritatively set `artwork_path` for one or more tracks by ID, within a caller-supplied
/// transaction. Unlike `update_track_metadata`'s `artwork_path = COALESCE(?, artwork_path)`, this
/// is a plain overwrite — so `None` genuinely nulls the column, the artwork-Remove case a COALESCE
/// can never express. Tx-scoped because the tag-edit orchestrator writes it in the same
/// transaction as the metadata refresh.
pub async fn set_track_artwork(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    // Reserve 1 bind slot for the `artwork_path` parameter itself.
    for chunk in ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("UPDATE tracks SET artwork_path = ? WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(artwork_path);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Batch-update `file_hash` and `date_modified` for tracks by ID.
/// All chunks share a single transaction to amortize fsync under
/// `synchronous=NORMAL` — fast for retroactive backfills on large libraries.
///
/// Each chunk runs as a single CTE-driven UPDATE so the round-trip count
/// is O(chunks), not O(rows). Chunk size is [`MAX_BINDS_PER_STATEMENT`] divided
/// by 3 columns per row.
///
/// [`MAX_BINDS_PER_STATEMENT`]: crate::database::MAX_BINDS_PER_STATEMENT
pub async fn batch_update_hashes(
    db: &DbPool,
    updates: &[(i64, String, Option<String>)],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 3;
    const CHUNK_SIZE: usize = crate::database::MAX_BINDS_PER_STATEMENT / COLS_PER_ROW; // 333

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
