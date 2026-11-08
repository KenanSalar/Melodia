//! Writes to track rows: counters, the per-row flags, artwork and hashes.

use rusqlite::Transaction;

use crate::database::sql::{self, Sql};
use crate::database::{
    CASE_BINDS_PER_ROW, DbPool, MAX_BINDS_PER_STATEMENT, case_by_id, chunked_in_query,
    chunked_in_statements, placeholders, values_placeholders,
};
use melodia_core::error::AppError;

pub async fn update_play_count(db: &DbPool, id: i64) -> Result<(), AppError> {
    let now = melodia_core::utils::now_rfc3339();
    db.write(move |conn| {
        sql::execute(
            conn,
            "UPDATE tracks SET play_count = play_count + 1, last_played = ? WHERE id = ?",
            (now, id),
        )
    })
    .await?;
    Ok(())
}

pub async fn update_skip_count(db: &DbPool, id: i64) -> Result<(), AppError> {
    db.write(move |conn| {
        sql::execute(conn, "UPDATE tracks SET skip_count = skip_count + 1 WHERE id = ?", (id,))
    })
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
    let max_rows =
        (MAX_BINDS_PER_STATEMENT - usize::from(last_played.is_some())) / CASE_BINDS_PER_ROW;
    let stamp = if last_played.is_some() { ", last_played = ?" } else { "" };

    for chunk in increments.chunks(max_rows) {
        let mut sql = Sql::trusted(format!(
            "UPDATE tracks SET {column} = {column} + {}{stamp} WHERE id IN ({})",
            case_by_id(chunk.len()),
            placeholders(chunk.len()),
        ));
        for &(id, n) in chunk {
            sql.bind(id).bind(i64::from(n));
        }
        if let Some(now) = last_played {
            sql.bind(now);
        }
        for &(id, _) in chunk {
            sql.bind(id);
        }
        db.write(move |conn| sql.execute(conn)).await?;
    }
    Ok(())
}

pub async fn update_last_position(db: &DbPool, id: i64, position_ms: i64) -> Result<(), AppError> {
    db.write(move |conn| {
        sql::execute(conn, "UPDATE tracks SET last_position = ? WHERE id = ?", (position_ms, id))
    })
    .await?;
    Ok(())
}

pub async fn set_favorite(db: &DbPool, ids: &[i64], favorite: bool) -> Result<(), AppError> {
    let updates = chunked_in_statements(favorite, ids, |placeholders| {
        format!("UPDATE tracks SET is_favorite = ? WHERE id IN ({placeholders})")
    });
    for sql in updates {
        db.write(move |conn| sql.execute(conn)).await?;
    }
    Ok(())
}

/// Clears the heart on those of `ids` that carry one, answering exactly those, so an undo puts
/// back only what this took. One transaction: a failed chunk leaves nothing cleared that the
/// answer doesn't name.
pub async fn unfavorite(db: &DbPool, ids: Vec<i64>) -> Result<Vec<i64>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    db.transaction(move |tx| {
        chunked_in_query(tx, &ids, |placeholders| {
            format!(
                "UPDATE tracks SET is_favorite = 0 WHERE is_favorite = 1 AND id IN ({placeholders})
                 RETURNING id"
            )
        })
    })
    .await
}

/// Sets the star `rating`, which `library::ratings::set_rating` has already clamped.
pub async fn set_rating(db: &DbPool, ids: &[i64], rating: i32) -> Result<(), AppError> {
    let updates = chunked_in_statements(rating, ids, |placeholders| {
        format!("UPDATE tracks SET rating = ? WHERE id IN ({placeholders})")
    });
    for sql in updates {
        db.write(move |conn| sql.execute(conn)).await?;
    }
    Ok(())
}

/// Overwrites `artwork_path` inside the tag edit's transaction, so `None` nulls it as
/// `update_track_metadata`'s `COALESCE` can't.
pub(crate) fn set_track_artwork(
    tx: &Transaction<'_>,
    ids: &[i64],
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    let updates = chunked_in_statements(artwork_path, ids, |placeholders| {
        format!("UPDATE tracks SET artwork_path = ? WHERE id IN ({placeholders})")
    });
    for update in updates {
        update.execute(tx)?;
    }
    Ok(())
}

/// Blanks every `date_modified`, answering how many rows moved. NULL honestly means unknown: it
/// matches no file's mtime in `file_is_current`, and the scan refills it from the `stat` that
/// proved the file exists.
pub async fn mark_every_track_stale(db: &DbPool) -> Result<u64, AppError> {
    db.write(|conn| {
        sql::execute(
            conn,
            "UPDATE tracks SET date_modified = NULL WHERE date_modified IS NOT NULL",
            (),
        )
    })
    .await
}

/// Writes `(id, file_hash, date_modified)` triples, a CTE-driven UPDATE per chunk inside one
/// transaction, so a backfill over a large library pays one commit.
pub async fn batch_update_hashes(
    db: &DbPool,
    updates: Vec<(i64, String, Option<String>)>,
) -> Result<(), AppError> {
    if updates.is_empty() {
        return Ok(());
    }
    db.transaction(move |tx| set_hashes(tx, &updates)).await
}

fn set_hashes(
    tx: &Transaction<'_>,
    updates: &[(i64, String, Option<String>)],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 3;
    const CHUNK_SIZE: usize = MAX_BINDS_PER_STATEMENT / COLS_PER_ROW;

    for chunk in updates.chunks(CHUNK_SIZE) {
        let rows = values_placeholders(chunk.len(), COLS_PER_ROW);
        let mut sql = Sql::trusted(format!(
            "WITH v(id, h, m) AS (VALUES {rows})
             UPDATE tracks
                SET file_hash = (SELECT h FROM v WHERE v.id = tracks.id),
                    date_modified = (SELECT m FROM v WHERE v.id = tracks.id)
              WHERE id IN (SELECT id FROM v)"
        ));
        for (id, hash, mtime) in chunk {
            sql.bind(*id).bind(hash.as_str()).bind(mtime.as_deref());
        }
        sql.execute(tx)?;
    }
    Ok(())
}
