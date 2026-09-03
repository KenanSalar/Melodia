//! Putting a station on the deck, and the row bookkeeping that goes with it.
//!
//! **A play writes the row before it opens the socket.** Both doors below reach the same
//! [`keep_station`], because a station the user chose has to reach the recents list whether or not
//! it was reachable today — a station that is down is exactly the one they will want to find again.

use std::sync::Arc;

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::library::playback;
use crate::player::types::RadioNowPlaying;
use crate::services::radio_browser;
use crate::state::AppState;

use super::{
    directory_client, ensure_enabled, get_station, save_station, set_artwork, set_favorite,
};

/// Count a play against a station, which is what orders the recents list.
pub async fn mark_played(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::radio::mark_played(&state.db, id).await
}

/// Tune to a stored station, counting the play ahead of the connect.
///
/// The count goes in even if the stream turns out to be unreachable: it records that the user
/// chose the station, which is what orders the recents list, and a station that is down today is
/// exactly the one they will want to find again. Hence the ordering — counting afterwards would
/// make the row conditional on the server being up.
pub async fn play_station(state: &AppState, id: i64) -> Result<(), AppError> {
    ensure_enabled(state)?;
    let mut station = get_station(state, id).await?;
    mark_played(state, id).await?;
    // The row was read before the count went in, and this play is one the Now-Playing surfaces
    // should already be stating. `mark_played` is `play_count + 1`, so this is the new value
    // rather than a guess at it.
    station.play_count += 1;
    let now_playing = std::sync::Arc::new(RadioNowPlaying::from(&station));
    // Every play passes through here, whichever surface started it, so the directory's own
    // count is reported once and in one place rather than at each caller.
    spawn_click(state, station.station_uuid.as_deref());
    playback::player_play_station(&state.playback_ctx(), &now_playing).await
}

/// The station a restart should put back on the deck, or nothing.
///
/// Guarded like a play rather than like a getter: what comes back goes on a Now-Playing surface
/// with a button that opens a socket, which is what D15's switch is about.
///
/// Silent on every miss — radio switched off since, or a row removed from both tabs and swept.
/// Nothing has been asked for at this point in the boot, so there is nothing to report to
/// somebody who may not have been thinking about the station at all.
pub async fn station_to_restore(state: &AppState, id: i64) -> Option<Arc<RadioNowPlaying>> {
    if !state.radio_enabled.get() {
        return None;
    }
    match get_station(state, id).await {
        Ok(station) => Some(Arc::new(RadioNowPlaying::from(&station))),
        Err(e) => {
            log::debug!("radio: not restoring station {id}: {}", crate::error::describe(&e));
            None
        }
    }
}

/// Write a browsed station into the table, which is what makes it the user's.
///
/// Directory results are otherwise never persisted (D3), so this is the crossing, and both of
/// the things that count as keeping a station go through it.
///
/// **The logo write follows the fetched URL, not the stored file.** `save_station`'s conflict
/// clause deliberately leaves `artwork_path` alone, which is right for a re-import that changed
/// nothing else and wrong for a station whose logo moved: the caller fetched from the
/// `favicon_url` in hand, so pointing the row at what that returned is what stops a moved logo
/// showing the old one forever.
async fn keep_station(
    state: &AppState,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<i64, AppError> {
    let id = save_station(state, &station.to_new_station()).await?;
    if logo.is_some() {
        set_artwork(state, id, logo).await?;
    }
    Ok(id)
}

/// Keep or release a browsed station, writing its row on the way in.
///
/// Un-favoriting leaves the row: it may carry a play history, and deciding whether an unstarred
/// never-played station is worth deleting belongs with the surface that lists them.
pub async fn set_directory_favorite(
    state: &AppState,
    station: &radio::DirectoryStation,
    favorite: bool,
    logo: Option<&str>,
) -> Result<i64, AppError> {
    let id = keep_station(state, station, logo).await?;
    set_favorite(state, id, favorite).await?;
    Ok(id)
}

/// Tune to a browsed station, keeping it first.
pub async fn play_directory_station(
    state: &AppState,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<(), AppError> {
    ensure_enabled(state)?;
    let id = keep_station(state, station, logo).await?;
    play_station(state, id).await
}

/// Tell the directory a station was played, if the user left that on.
///
/// Detached rather than awaited: the click is the directory's business and the listener's is the
/// audio, so a slow mirror must not sit between the click and the connect. Failures are a debug
/// line — there is nothing for a user to do about one, and the call is deduplicated server-side
/// so a repeat is not an error either.
fn spawn_click(state: &AppState, station_uuid: Option<&str>) {
    if !state.radio_send_clicks.get() {
        return;
    }
    let Some(uuid) = station_uuid.filter(|uuid| !uuid.is_empty()) else {
        return;
    };
    let (s, uuid) = (state.clone(), uuid.to_owned());
    state.runtime.spawn(async move {
        let Ok(client) = directory_client(&s) else {
            return;
        };
        if let Err(e) = radio_browser::count_click(client, &uuid).await {
            log::debug!("radio: click report failed: {}", crate::error::describe(&e));
        }
    });
}
