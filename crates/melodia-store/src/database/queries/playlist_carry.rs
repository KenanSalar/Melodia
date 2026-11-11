//! A track's playlist places handed to others where the library replaces it: a cue sheet splitting
//! an image into its tracks or giving the image back, and a removed duplicate's going to the copy
//! kept. Neither is the user editing a playlist, so its `updated_at`, and with it its card's place,
//! stays.

use std::collections::HashMap;

use rusqlite::Transaction;

use super::playlist::{NewItem, contiguous, insert_items_tx, renumber_playlist_positions_tx};
use crate::database::sql;
use crate::database::{chunked_in_execute, chunked_in_query};
use melodia_core::error::AppError;

/// A playlist's entry for a track a scan replaces: the playlist, the position, and when it was added.
type Place = (i64, i32, String);

/// Puts `new_ids`, in order, where `old_id` sits in each playlist holding it, and takes `old_id`
/// out. The new entries keep the old one's `added_at`.
pub(crate) fn replace_in_playlists(
    tx: &Transaction<'_>,
    old_id: i64,
    new_ids: &[i64],
) -> Result<(), AppError> {
    let places: Vec<Place> = sql::fetch_all(
        tx,
        "SELECT playlist_id, position, added_at FROM playlist_items WHERE track_id = ?",
        (old_id,),
    )?;
    // At most a sheet's tracks, so it fits.
    let room = i32::try_from(new_ids.len()).unwrap_or(i32::MAX) - 1;
    for (playlist_id, position, added_at) in places {
        sql::execute(
            tx,
            "DELETE FROM playlist_items WHERE playlist_id = ? AND track_id = ?",
            (playlist_id, old_id),
        )?;
        sql::execute(
            tx,
            "UPDATE playlist_items SET position = position + ? WHERE playlist_id = ? AND position > ?",
            (room, playlist_id, position),
        )?;
        insert_items_tx(tx, playlist_id, &contiguous(new_ids, position, &added_at)?)?;
    }
    Ok(())
}

/// Gives each keeper its spares' playlist places, at its first spare's position and with that
/// spare's added date, and takes the spares' own entries out. An insert rather than a re-point,
/// since the playlist stats triggers watch only inserts and deletes; a playlist the keeper is
/// already in keeps the keeper's own place.
pub(crate) fn move_playlist_places(
    tx: &Transaction<'_>,
    keeper_of: &HashMap<i64, i64>,
) -> Result<(), AppError> {
    let spare_ids: Vec<i64> = keeper_of.keys().copied().collect();
    let mut places: Vec<(i64, i64, i32, String)> =
        chunked_in_query(tx, &spare_ids, |placeholders| {
            format!(
                "SELECT playlist_id, track_id, position, added_at FROM playlist_items \
                 WHERE track_id IN ({placeholders})"
            )
        })?;
    // Lowest first, since the first insert wins; in Rust, as an ORDER BY holds per chunk only.
    places.sort_unstable_by_key(|&(_, _, position, _)| position);

    let mut by_playlist: HashMap<i64, Vec<NewItem<'_>>> = HashMap::new();
    for (playlist_id, spare_id, position, added_at) in &places {
        let Some(&keeper) = keeper_of.get(spare_id) else {
            continue;
        };
        by_playlist.entry(*playlist_id).or_default().push(NewItem {
            track_id: keeper,
            position: *position,
            added_at,
        });
    }
    for (playlist_id, items) in &by_playlist {
        insert_items_tx(tx, *playlist_id, items)?;
    }
    chunked_in_execute(tx, &spare_ids, |placeholders| {
        format!("DELETE FROM playlist_items WHERE track_id IN ({placeholders})")
    })?;
    for playlist_id in by_playlist.keys() {
        renumber_playlist_positions_tx(tx, *playlist_id)?;
    }
    Ok(())
}
