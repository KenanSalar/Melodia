//! Taking stations out of a kept tab, and putting back what that took. Each is one transaction, so
//! whether a row is still listed is decided on the row as the removal leaves it.

use sqlx::AssertSqlSafe;

use super::radio::kept_station_matching;
use crate::database::{DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_query, placeholders};
use melodia_core::entities::radio::{KeptList, RadioStation, StationCut};
use melodia_core::error::AppError;

/// Drops stations out of `from`, deleting each row neither tab lists any more, and answers what
/// each lost. A station `from` doesn't list is left alone and answers nothing.
pub async fn remove_stations(
    db: &DbPool,
    ids: &[i64],
    from: KeptList,
) -> Result<Vec<StationCut>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = db.write().begin().await?;

    let cuts: Vec<StationCut> = stations_tx(&mut tx, ids)
        .await?
        .into_iter()
        .filter_map(|station| cut_from(station, from))
        .collect();
    let mut kept = Vec::new();
    let mut gone = Vec::new();
    for cut in &cuts {
        match cut {
            StationCut::Unfavorited { id, .. } | StationCut::HistoryCleared { id, .. } => {
                kept.push(*id);
            }
            StationCut::Deleted(station) => gone.push(station.id),
        }
    }

    // The count goes with the stamp: Favorites sorts on it, and a station showing seven plays and
    // no last-played is a history half-erased.
    let take = match from {
        KeptList::Favorites => "is_favorite = FALSE",
        KeptList::Recent => "last_played = NULL, play_count = 0",
    };
    execute_by_ids(&mut tx, &kept, |ids| {
        format!("UPDATE radio_stations SET {take} WHERE id IN ({ids})")
    })
    .await?;
    execute_by_ids(&mut tx, &gone, |ids| format!("DELETE FROM radio_stations WHERE id IN ({ids})"))
        .await?;

    tx.commit().await?;
    Ok(cuts)
}

/// What removing `station` from `from` takes, or `None` where `from` doesn't list it.
fn cut_from(station: RadioStation, from: KeptList) -> Option<StationCut> {
    let mut left = station.clone();
    let taken = match from {
        KeptList::Favorites if station.is_favorite => {
            left.is_favorite = false;
            StationCut::Unfavorited {
                id: station.id,
                date_added: station.date_added.clone(),
                name: station.name.clone(),
            }
        }
        KeptList::Favorites => return None,
        KeptList::Recent => {
            let last_played = station.last_played.clone()?;
            left.last_played = None;
            left.play_count = 0;
            StationCut::HistoryCleared {
                id: station.id,
                date_added: station.date_added.clone(),
                name: station.name.clone(),
                last_played,
                play_count: station.play_count,
            }
        }
    };
    Some(if left.is_listed() { taken } else { StationCut::Deleted(Box::new(station)) })
}

/// Deletes the row of `id` if neither local tab lists it.
pub async fn delete_if_unlisted(db: &DbPool, id: i64) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;
    let station = sqlx::query_as::<_, RadioStation>("SELECT * FROM radio_stations WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if station.is_some_and(|station| !station.is_listed()) {
        sqlx::query("DELETE FROM radio_stations WHERE id = ?").bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The stored logos `ids` name.
pub async fn station_artwork(db: &DbPool, ids: &[i64]) -> Result<Vec<String>, AppError> {
    let paths: Vec<(String,)> = chunked_in_query(db.read(), ids, |placeholders| {
        format!(
            "SELECT artwork_path FROM radio_stations
             WHERE artwork_path IS NOT NULL AND id IN ({placeholders})"
        )
    })
    .await?;
    Ok(paths.into_iter().map(|(path,)| path).collect())
}

/// Puts back what `cuts` took, `cuts` in the order they were made, undone newest first. Each
/// reverses only its own removal: a play or a favorite since stays, a row gone since stays gone, and
/// a station kept again from Browse isn't written twice.
pub async fn restore_stations(db: &DbPool, cuts: &[StationCut]) -> Result<(), AppError> {
    if cuts.is_empty() {
        return Ok(());
    }
    let mut tx = db.write().begin().await?;
    for cut in cuts.iter().rev() {
        match cut {
            // Both arms match `date_added`: SQLite re-issues the largest id once its row goes, and
            // Browse's un-heart can delete a row whose plays this undo holds without retiring it.
            StationCut::Unfavorited { id, date_added, .. } => {
                sqlx::query(
                    "UPDATE radio_stations SET is_favorite = TRUE WHERE id = ? AND date_added = ?",
                )
                .bind(id)
                .bind(date_added)
                .execute(&mut *tx)
                .await?;
            }
            StationCut::HistoryCleared { id, date_added, last_played, play_count, .. } => {
                // `MAX` answers NULL when either side is, so a station unplayed since takes the
                // held stamp.
                sqlx::query(
                    "UPDATE radio_stations
                     SET play_count = play_count + ?, last_played = COALESCE(MAX(last_played, ?), ?)
                     WHERE id = ? AND date_added = ?",
                )
                .bind(play_count)
                .bind(last_played)
                .bind(last_played)
                .bind(id)
                .bind(date_added)
                .execute(&mut *tx)
                .await?;
            }
            StationCut::Deleted(station) => reinsert_tx(&mut tx, station).await?,
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Writes a deleted row back whole under its own id, or a fresh one where another station took
/// it, unless the station is back already: kept again from Browse, or its URL typed in again.
async fn reinsert_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    station: &RadioStation,
) -> Result<(), AppError> {
    let back =
        kept_station_matching(&mut **tx, station.station_uuid.as_deref(), &station.stream_url)
            .await?;
    if back.is_some() {
        return Ok(());
    }
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM radio_stations WHERE id = ?)")
            .bind(station.id)
            .fetch_one(&mut **tx)
            .await?;
    // A NULL `INTEGER PRIMARY KEY` takes the next free id.
    let id = (!taken).then_some(station.id);

    sqlx::query(
        "INSERT INTO radio_stations (
             id, station_uuid, name, stream_url, homepage, local_homepage, favicon_url,
             local_favicon_url, local_tags, local_country, artwork_path, tags, country,
             country_code, language, codec, probed_codec, bitrate, hls, is_favorite, sort_key,
             date_added, last_played, play_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(&station.station_uuid)
    .bind(&station.name)
    .bind(&station.stream_url)
    .bind(&station.homepage)
    .bind(&station.local_homepage)
    .bind(&station.favicon_url)
    .bind(&station.local_favicon_url)
    .bind(&station.local_tags)
    .bind(&station.local_country)
    .bind(&station.artwork_path)
    .bind(&station.tags)
    .bind(&station.country)
    .bind(&station.country_code)
    .bind(&station.language)
    .bind(&station.codec)
    .bind(&station.probed_codec)
    .bind(station.bitrate)
    .bind(station.hls)
    .bind(station.is_favorite)
    .bind(&station.sort_key)
    .bind(&station.date_added)
    .bind(&station.last_played)
    .bind(station.play_count)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Every row of `ids` still there.
async fn stations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
) -> Result<Vec<RadioStation>, AppError> {
    let mut stations = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(MAX_BINDS_PER_STATEMENT) {
        let sql =
            format!("SELECT * FROM radio_stations WHERE id IN ({})", placeholders(chunk.len()));
        let mut query = sqlx::query_as::<_, RadioStation>(AssertSqlSafe(sql)).persistent(false);
        for &id in chunk {
            query = query.bind(id);
        }
        stations.extend(query.fetch_all(&mut **tx).await?);
    }
    Ok(stations)
}

/// `statement` is handed each chunk's placeholder list.
async fn execute_by_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
    statement: impl Fn(&str) -> String,
) -> Result<(), AppError> {
    for chunk in ids.chunks(MAX_BINDS_PER_STATEMENT) {
        let mut query =
            sqlx::query(AssertSqlSafe(statement(&placeholders(chunk.len())))).persistent(false);
        for &id in chunk {
            query = query.bind(id);
        }
        query.execute(&mut **tx).await?;
    }
    Ok(())
}
