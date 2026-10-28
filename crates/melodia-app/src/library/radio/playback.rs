//! Putting a station on the deck, and the row bookkeeping that goes with it.

use std::sync::Arc;

use crate::library::playback;
use crate::state::AppState;
use melodia_core::entities::radio;
use melodia_core::error::{AppError, describe};
use melodia_engine::player::engine::types::RadioNowPlaying;
use melodia_net::services::net::radio_browser;
use melodia_store::database::{DbPool, queries};

use super::{directory_client, ensure_enabled, get_station};

/// Count a play against a station, which is what orders the recents list.
pub async fn mark_played(db: &DbPool, id: i64) -> Result<(), AppError> {
    queries::radio::mark_played(db, id).await
}

/// Write down the codec the open mount actually serves, which only the decoder can tell: the
/// directory's `codec` names the container. Its own column, a re-import rewriting the directory's.
///
/// **Last, once the stream plays**: anywhere earlier, a write-pool `UPDATE` sits between the open
/// and the first sample. `station_id` is `0` for a station with no row of its own.
pub async fn record_probed_codec(db: &DbPool, station: &RadioNowPlaying, codec: &str) {
    if codec.is_empty() || station.station_id == 0 {
        return;
    }
    if let Err(e) = queries::radio::set_probed_codec(db, station.station_id, codec).await {
        log::warn!("Could not record {}'s format: {}", station.name, describe(&e));
    }
}

/// Tune to a stored station, counting the play ahead of the connect.
///
/// **The count goes in even if the stream is unreachable**: a station that is down today is
/// exactly the one the user will want to find in the recents list again.
pub async fn play_station(state: &AppState, id: i64) -> Result<(), AppError> {
    ensure_enabled(state)?;
    let mut station = get_station(&state.db, id).await?;
    mark_played(&state.db, id).await?;
    // Read before the count went in; `mark_played` is `play_count + 1`, so this is the new value.
    station.play_count += 1;
    let now_playing = std::sync::Arc::new(RadioNowPlaying::from(&station));
    // Every play passes through here, so the directory's count is reported in one place.
    spawn_click(state, station.station_uuid.as_deref());
    playback::player_play_station(&state.playback_ctx(), &now_playing).await
}

/// The station a restart should put back on the deck, or nothing. Guarded like a play rather than
/// like a getter, what comes back going on a surface with a button that opens a socket.
pub async fn station_to_restore(state: &AppState, id: i64) -> Option<Arc<RadioNowPlaying>> {
    ensure_enabled(state).ok()?;
    match get_station(&state.db, id).await {
        Ok(station) => Some(Arc::new(RadioNowPlaying::from(&station))),
        Err(e) => {
            log::debug!("radio: not restoring station {id}: {}", melodia_core::error::describe(&e));
            None
        }
    }
}

/// Write a browsed station into the table, the one crossing from a directory answer to a row.
/// **The logo write follows the fetched URL**: `save_station`'s conflict clause leaves
/// `artwork_path` alone, so a station whose logo moved would keep showing the old one.
async fn keep_station(
    db: &DbPool,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<i64, AppError> {
    let id = queries::radio::save_station(db, &station.to_new_station()).await?;
    if logo.is_some() {
        queries::radio::set_artwork(db, id, logo).await?;
    }
    Ok(id)
}

/// Keep or release a browsed station, writing its row on the way in. Un-favoriting leaves the row,
/// whether to delete it being [`super::delete_if_unlisted`]'s call.
pub async fn set_directory_favorite(
    db: &DbPool,
    station: &radio::DirectoryStation,
    favorite: bool,
    logo: Option<&str>,
) -> Result<i64, AppError> {
    let id = keep_station(db, station, logo).await?;
    queries::radio::set_favorite(db, id, favorite).await?;
    Ok(id)
}

/// Tune to a browsed station, keeping it first.
pub async fn play_directory_station(
    state: &AppState,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<(), AppError> {
    ensure_enabled(state)?;
    let id = keep_station(&state.db, station, logo).await?;
    play_station(state, id).await
}

/// Tell the directory a station was played, if the user left that on. Detached, so a slow mirror
/// never sits between the click and the connect.
fn spawn_click(state: &AppState, station_uuid: Option<&str>) {
    let Some(uuid) = click_uuid(state.radio_send_clicks.get(), station_uuid) else {
        return;
    };
    let (s, uuid) = (state.clone(), uuid.to_owned());
    state.runtime.spawn(async move {
        let Ok(client) = directory_client(&s) else {
            return;
        };
        if let Err(e) = radio_browser::count_click(client, &uuid).await {
            log::debug!("radio: click report failed: {}", melodia_core::error::describe(&e));
        }
    });
}

/// The uuid a play reports to the directory, or `None`: the setting is off, or the station is
/// hand-typed (no uuid, or an empty one) and names nothing the directory could count.
fn click_uuid(send_clicks: bool, station_uuid: Option<&str>) -> Option<&str> {
    if !send_clicks {
        return None;
    }
    station_uuid.filter(|uuid| !uuid.is_empty())
}

#[cfg(test)]
#[path = "../tests/radio_playback_tests.rs"]
mod tests;
