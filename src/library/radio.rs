//! Radio library API, and the only door the UI has onto stations.
//!
//! The stored table and the radio-browser.info directory answer through the same
//! module rather than side by side, so a callback never has to know which one
//! did, and so the toggle that turns radio off has one place to guard.
//!
//! Writes here deliberately do not bump `library_changed_tx`. Its subscribers
//! are the library views, none of which shows a station; the Radio section
//! refreshes through its own global.

use std::sync::Arc;

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::library::playback;
use crate::player::types::RadioNowPlaying;
use crate::services::radio_browser;
use crate::state::AppState;

/// How far back the recently-played station list reaches.
///
/// A station history has no natural end, so this is what bounds the fetch. The
/// list is a way back to what you were just listening to rather than a record,
/// so it is sized to a session or two of tuning around, not to everything ever
/// played.
pub const RECENT_STATIONS_LIMIT: i64 = 50;

/// Every favorited station, naturally name-ordered.
pub async fn get_favorites(state: &AppState) -> Result<Vec<radio::RadioStation>, AppError> {
    queries::radio::get_favorite_stations(&state.db).await
}

/// The stations played most recently, newest first.
pub async fn get_recent(state: &AppState) -> Result<Vec<radio::RadioStation>, AppError> {
    queries::radio::get_recent_stations(&state.db, RECENT_STATIONS_LIMIT).await
}

/// One station, or `AppError::NotFound` if it is gone.
pub async fn get_station(state: &AppState, id: i64) -> Result<radio::RadioStation, AppError> {
    queries::radio::get_station_by_id(&state.db, id).await
}

/// Persist a station, updating the row when the directory already knows it.
/// Preserves everything the user did with it.
pub async fn save_station(
    state: &AppState,
    station: &radio::NewRadioStation,
) -> Result<radio::RadioStation, AppError> {
    queries::radio::save_station(&state.db, station).await
}

pub async fn set_favorite(state: &AppState, id: i64, favorite: bool) -> Result<(), AppError> {
    queries::radio::set_favorite(&state.db, id, favorite).await
}

pub async fn remove_station(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::radio::delete_station(&state.db, id).await
}

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
    let station = get_station(state, id).await?;
    let now_playing = RadioNowPlaying::from(&station);
    mark_played(state, id).await?;
    playback::player_play_station(&state.playback_ctx(), &now_playing).await
}

/// Point a station at its stored logo, or clear it with `None`.
pub async fn set_artwork(
    state: &AppState,
    id: i64,
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    queries::radio::set_artwork(&state.db, id, artwork_path).await
}

/// Search the directory. Results are a network answer with a shelf life and are
/// never written to the table; one becomes a row when the user keeps or plays it.
pub async fn search(
    state: &AppState,
    search: &radio::StationSearch,
) -> Result<Vec<radio::DirectoryStation>, AppError> {
    radio_browser::search(state.http_client(), search).await
}

/// One of the directory's facet lists, for the filter chips. Large and
/// near-static, so it is fetched once per session and shared thereafter.
pub async fn facets(
    state: &AppState,
    kind: radio::FacetKind,
) -> Result<Arc<[radio::Facet]>, AppError> {
    radio_browser::facets(state.http_client(), kind).await
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
