//! Radio library API, and the only door the UI has onto stations.
//!
//! The stored table and the radio-browser.info directory answer through the same
//! module rather than side by side, so a callback never has to know which one
//! did, and so the toggle that turns radio off has one place to guard.
//!
//! Writes here deliberately do not bump `library_changed_tx`. Its subscribers
//! are the library views, none of which shows a station; the Radio section
//! refreshes through its own global.

use std::collections::HashSet;
use std::sync::Arc;

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::library::playback;
use crate::player::types::RadioNowPlaying;
use crate::services::radio_browser;
use crate::state::AppState;

/// Stations per directory page.
///
/// Re-exported rather than restated so a caller can page without naming the client: an offset
/// advances by exactly the limit the request carried, and the two coming from one definition is
/// what stops paging skipping or repeating a page.
pub use radio_browser::DEFAULT_PAGE_LIMIT;

/// How far back the recently-played station list reaches.
///
/// A station history has no natural end, so this is what bounds the fetch. The
/// list is a way back to what you were just listening to rather than a record,
/// so it is sized to a session or two of tuning around, not to everything ever
/// played.
pub const RECENT_STATIONS_LIMIT: i64 = 50;

/// A refusal when the user has switched Radio off.
///
/// **This is where "off" means no traffic.** D14 already made this module the only door
/// onto the directory, so the switch is enforced here rather than at the sidebar row,
/// which a stale callback or an in-flight fetch is already past. Stored stations stay
/// readable through the getters below: hiding a feature is not deleting what the user
/// kept, and nothing but the section itself asks for them.
fn ensure_enabled(state: &AppState) -> Result<(), AppError> {
    if state.radio_enabled() {
        return Ok(());
    }
    Err(AppError::Settings("Radio is switched off".to_owned()))
}

/// The shared client, reachable only past [`ensure_enabled`] — which is the point of
/// spelling it as a seam rather than repeating the check beside each call.
///
/// Every outbound call this module makes takes it, including the logo download, whose host
/// is one the directory named rather than the directory itself. The guard is about traffic,
/// not about who is on the other end.
fn directory_client(state: &AppState) -> Result<&reqwest::Client, AppError> {
    ensure_enabled(state)?;
    Ok(state.http_client())
}

/// Every favorited station, naturally name-ordered.
pub async fn get_favorites(state: &AppState) -> Result<Vec<radio::RadioStation>, AppError> {
    queries::radio::get_favorite_stations(&state.db).await
}

/// The directory uuids of every favorited station, which is what a browsed row
/// is matched against to decide whether its star is filled.
///
/// A projection of [`get_favorites`] rather than a query of its own: the table
/// is one the user fills by hand, so the rows are few and a second statement
/// would be a second thing to keep true. Hand-typed stations carry no uuid and
/// drop out, correctly — nothing in the directory can match one.
pub async fn favorite_uuids(state: &AppState) -> Result<HashSet<String>, AppError> {
    Ok(get_favorites(state).await?.into_iter().filter_map(|s| s.station_uuid).collect())
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
    ensure_enabled(state)?;
    let station = get_station(state, id).await?;
    let now_playing = RadioNowPlaying::from(&station);
    mark_played(state, id).await?;
    // Every play passes through here, whichever surface started it, so the directory's own
    // count is reported once and in one place rather than at each caller.
    spawn_click(state, station.station_uuid.as_deref());
    playback::player_play_station(&state.playback_ctx(), &now_playing).await
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
) -> Result<radio::RadioStation, AppError> {
    let saved = save_station(state, &station.to_new_station()).await?;
    if logo.is_some() {
        set_artwork(state, saved.id, logo).await?;
    }
    Ok(saved)
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
    let saved = keep_station(state, station, logo).await?;
    set_favorite(state, saved.id, favorite).await?;
    Ok(saved.id)
}

/// Tune to a browsed station, keeping it first.
///
/// The refusal is the honest half of D13: the card already says an HLS station cannot play, and
/// this is what stops a stale row or a second surface reaching the decoder with a stream
/// Symphonia has no demuxer for. Checked before the row is written, so a click that cannot
/// succeed also does not leave a station behind.
pub async fn play_directory_station(
    state: &AppState,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<(), AppError> {
    ensure_enabled(state)?;
    if station.hls {
        return Err(AppError::Validation(
            "This station streams in a segmented format Melodia cannot play yet".to_owned(),
        ));
    }
    let saved = keep_station(state, station, logo).await?;
    play_station(state, saved.id).await
}

/// Tell the directory a station was played, if the user left that on.
///
/// Detached rather than awaited: the click is the directory's business and the listener's is the
/// audio, so a slow mirror must not sit between the click and the connect. Failures are a debug
/// line — there is nothing for a user to do about one, and the call is deduplicated server-side
/// so a repeat is not an error either.
fn spawn_click(state: &AppState, station_uuid: Option<&str>) {
    if !state.radio_send_clicks() {
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
            log::debug!("radio: click report failed: {}", crate::services::describe(&e));
        }
    });
}

/// Fetch a station's logo into the shared artwork store, returning its path there.
///
/// Through the same seam as the directory calls: "off" has to mean no traffic, and a logo
/// download is traffic whoever is serving it.
pub async fn fetch_logo(state: &AppState, favicon_url: &str) -> Result<Option<String>, AppError> {
    let client = directory_client(state)?;
    crate::media::station_logo::fetch(client, favicon_url, &state.paths.artwork_dir).await
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
) -> Result<radio::StationPage, AppError> {
    let mut page = radio_browser::search(directory_client(state)?, search).await?;
    hide_hls(&mut page, state.radio_hide_hls());
    Ok(page)
}

/// Drop the segmented stations from a page, if the user has them hidden.
///
/// Here rather than in the request because the endpoint has no `hls` parameter to send. It thins
/// the page without touching [`radio::StationPage::has_more`], which the client already read off
/// the raw response: these rows were served and counted, and paging has to step over them rather
/// than stop at them.
fn hide_hls(page: &mut radio::StationPage, hide: bool) {
    if hide {
        page.stations.retain(|station| !station.hls);
    }
}

/// One of the directory's facet lists, for the filter chips. Large and
/// near-static, so it is fetched once per session and shared thereafter.
pub async fn facets(
    state: &AppState,
    kind: radio::FacetKind,
) -> Result<Arc<[radio::Facet]>, AppError> {
    radio_browser::facets(directory_client(state)?, kind).await
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
