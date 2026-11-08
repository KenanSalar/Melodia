//! Taking stations out of a kept tab, and putting back what that took. Each is one transaction, so
//! whether a row is still listed is decided on the row as the removal leaves it.

use rusqlite::{Transaction, params};

use super::radio::kept_station_matching;
use crate::database::{DbPool, chunked_in_execute, chunked_in_query, sql};
use melodia_core::entities::radio::{KeptList, RadioStation, StationCut};
use melodia_core::error::AppError;

/// Drops stations out of `from`, deleting each row neither tab lists any more, and answers what
/// each lost. A station `from` doesn't list is left alone and answers nothing.
pub async fn remove_stations(
    db: &DbPool,
    ids: Vec<i64>,
    from: KeptList,
) -> Result<Vec<StationCut>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    db.transaction(move |tx| cut_stations(tx, &ids, from)).await
}

fn cut_stations(
    tx: &Transaction<'_>,
    ids: &[i64],
    from: KeptList,
) -> Result<Vec<StationCut>, AppError> {
    let stations: Vec<RadioStation> = chunked_in_query(tx, ids, |placeholders| {
        format!("SELECT * FROM radio_stations WHERE id IN ({placeholders})")
    })?;
    let cuts: Vec<StationCut> =
        stations.into_iter().filter_map(|station| cut_from(station, from)).collect();
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
    chunked_in_execute(tx, &kept, |ids| {
        format!("UPDATE radio_stations SET {take} WHERE id IN ({ids})")
    })?;
    chunked_in_execute(tx, &gone, |ids| format!("DELETE FROM radio_stations WHERE id IN ({ids})"))?;
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
    db.transaction(move |tx| {
        let station: Option<RadioStation> =
            sql::fetch_optional(tx, "SELECT * FROM radio_stations WHERE id = ?", (id,))?;
        if station.is_some_and(|station| !station.is_listed()) {
            sql::execute(tx, "DELETE FROM radio_stations WHERE id = ?", (id,))?;
        }
        Ok(())
    })
    .await
}

/// The stored logos `ids` name.
pub async fn station_artwork(db: &DbPool, ids: &[i64]) -> Result<Vec<String>, AppError> {
    let ids = ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &ids, |placeholders| {
            format!(
                "SELECT artwork_path FROM radio_stations
                 WHERE artwork_path IS NOT NULL AND id IN ({placeholders})"
            )
        })
    })
    .await
}

/// Puts back what `cuts` took, `cuts` in the order they were made, undone newest first. Each
/// reverses only its own removal: a play or a favorite since stays, a row gone since stays gone, and
/// a station kept again from Browse isn't written twice.
pub async fn restore_stations(db: &DbPool, cuts: Vec<StationCut>) -> Result<(), AppError> {
    if cuts.is_empty() {
        return Ok(());
    }
    db.transaction(move |tx| put_back_stations(tx, &cuts)).await
}

fn put_back_stations(tx: &Transaction<'_>, cuts: &[StationCut]) -> Result<(), AppError> {
    for cut in cuts.iter().rev() {
        match cut {
            // Both arms match `date_added`: SQLite re-issues the largest id once its row goes, and
            // Browse's un-heart can delete a row whose plays this undo holds without retiring it.
            StationCut::Unfavorited { id, date_added, .. } => {
                sql::execute(
                    tx,
                    "UPDATE radio_stations SET is_favorite = TRUE WHERE id = ? AND date_added = ?",
                    (id, date_added),
                )?;
            }
            StationCut::HistoryCleared { id, date_added, last_played, play_count, .. } => {
                // `MAX` answers NULL when either side is, so a station unplayed since takes the
                // held stamp.
                sql::execute(
                    tx,
                    "UPDATE radio_stations
                     SET play_count = play_count + ?, last_played = COALESCE(MAX(last_played, ?), ?)
                     WHERE id = ? AND date_added = ?",
                    (play_count, last_played, last_played, id, date_added),
                )?;
            }
            StationCut::Deleted(station) => reinsert_tx(tx, station)?,
        }
    }
    Ok(())
}

/// Writes a deleted row back whole under its own id, or a fresh one where another station took
/// it, unless the station is back already: kept again from Browse, or its URL typed in again.
fn reinsert_tx(tx: &Transaction<'_>, station: &RadioStation) -> Result<(), AppError> {
    let back = kept_station_matching(tx, station.station_uuid.as_deref(), &station.stream_url)?;
    if back.is_some() {
        return Ok(());
    }
    let taken: bool = sql::fetch_one(
        tx,
        "SELECT EXISTS (SELECT 1 FROM radio_stations WHERE id = ?)",
        (station.id,),
    )?;
    // A NULL `INTEGER PRIMARY KEY` takes the next free id.
    let id = (!taken).then_some(station.id);

    sql::execute(
        tx,
        "INSERT INTO radio_stations (
             id, station_uuid, name, stream_url, homepage, local_homepage, favicon_url,
             local_favicon_url, local_tags, local_country, artwork_path, tags, country,
             country_code, language, codec, probed_codec, bitrate, hls, is_favorite, sort_key,
             date_added, last_played, play_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            station.station_uuid,
            station.name,
            station.stream_url,
            station.homepage,
            station.local_homepage,
            station.favicon_url,
            station.local_favicon_url,
            station.local_tags,
            station.local_country,
            station.artwork_path,
            station.tags,
            station.country,
            station.country_code,
            station.language,
            station.codec,
            station.probed_codec,
            station.bitrate,
            station.hls,
            station.is_favorite,
            station.sort_key,
            station.date_added,
            station.last_played,
            station.play_count,
        ],
    )?;
    Ok(())
}
