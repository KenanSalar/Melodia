//! A scan handing one track's playlist places to others: a cue sheet splitting an image into its
//! tracks, or giving the image back. A scan isn't the user editing a playlist, so its `updated_at`,
//! and with it its card's place, stays.

use rusqlite::Transaction;

use super::playlist::{contiguous, insert_items_tx, renumber_playlist_positions_tx};
use crate::database::sql;
use crate::database::{chunked_in_query, chunked_in_statements};
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

/// Puts `new_id` where the first of `old_ids` sits in each playlist holding any of them, and takes
/// them all out. The new entry keeps that first one's `added_at`.
pub(crate) fn merge_into_playlists(
    tx: &Transaction<'_>,
    old_ids: &[i64],
    new_id: i64,
) -> Result<(), AppError> {
    let mut places: Vec<Place> = chunked_in_query(tx, old_ids, |id_list| {
        format!(
            "SELECT playlist_id, position, added_at FROM playlist_items WHERE track_id IN ({id_list})"
        )
    })?;
    places.sort_unstable_by_key(|&(playlist_id, position, _)| (playlist_id, position));
    places.dedup_by_key(|&mut (playlist_id, _, _)| playlist_id);

    for (playlist_id, position, added_at) in places {
        let take_out = chunked_in_statements(playlist_id, old_ids, |id_list| {
            format!("DELETE FROM playlist_items WHERE playlist_id = ? AND track_id IN ({id_list})")
        });
        for statement in take_out {
            statement.execute(tx)?;
        }
        insert_items_tx(tx, playlist_id, &contiguous(&[new_id], position, &added_at)?)?;
        renumber_playlist_positions_tx(tx, playlist_id)?;
    }
    Ok(())
}
