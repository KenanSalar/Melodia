//! Radio library API, and the only door the UI has onto stations.
//!
//! The stored table and the radio-browser.info directory answer through the same
//! module rather than side by side, so a callback never has to know which one
//! did, and so the toggle that turns radio off has one place to guard.
//!
//! Writes here deliberately do not bump `library_changed`. Its subscribers
//! are the library views, none of which shows a station; the Radio section
//! refreshes through its own global.
//!
//! **The door is the module, not this file.** Four submodules answer for the four things a
//! station has — how it is authored, how it is played, where its logo comes from, and what the
//! directory says about it — and every one of them is re-exported here, so a caller still writes
//! `library::radio::play_station` and never learns which file answered. What stays in this file
//! is what all four need: the off switch, the client behind it, and the stored-table getters that
//! are exempt from both.
//!
//! **`ensure_enabled` and `directory_client` may not move.** The switch is enforced at the seam
//! rather than per call site, so a submodule reaching `state.http_client()` directly is traffic a
//! user who turned Radio off still pays. `library::radio::tests` walks *every* file under this
//! directory counting the reaches, which is what makes a fifth submodule covered by default.

mod authoring;
mod directory;
mod logos;
mod playback;

pub use authoring::{add_custom_station, set_station_overrides, update_custom_station};
pub use directory::{facets, search, station_details, vote};
pub use logos::{
    AnswerSeed, LogoAnswer, SiteOrigin, artwork_is_present, classify_logo_answer,
    discover_site_logo, fetch_logo, heal_seed_urls, heal_station_logo, logo_answers,
    prune_logo_answers, record_logo_outcome, set_artwork, site_origin,
};
pub use playback::{
    mark_played, play_directory_station, play_station, set_directory_favorite, station_to_restore,
};

// `library::radio_files` composes these with its own write rather than going through the door
// above — see the argument at each of their definitions.
pub(super) use authoring::{resolve_station_name, validated_overrides};

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::services::net::radio_browser;
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
pub(super) fn ensure_enabled(state: &AppState) -> Result<(), AppError> {
    if state.radio_enabled.get() {
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
pub(super) fn directory_client(state: &AppState) -> Result<&reqwest::Client, AppError> {
    ensure_enabled(state)?;
    Ok(state.http_client())
}

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

/// Persist a station, updating the row when the directory already knows it, and answer its id.
/// Preserves everything the user did with it.
pub async fn save_station(
    state: &AppState,
    station: &radio::NewRadioStation,
) -> Result<i64, AppError> {
    queries::radio::save_station(&state.db, station).await
}

pub async fn set_favorite(state: &AppState, id: i64, favorite: bool) -> Result<(), AppError> {
    queries::radio::set_favorite(&state.db, id, favorite).await
}

/// Drop a station out of the Favorites tab.
///
/// **Un-starring and deleting are the same button on two different rows.** A *directory* station
/// with a play behind it is still in Recently Played and its history is not the star's to take —
/// the argument [`set_directory_favorite`] already makes from the other side. One that was only
/// ever starred is listed nowhere once the star goes, and Browse rewrites it from the directory
/// the moment it is kept again. A hand-typed one has no directory to be rewritten from, so this
/// is its delete either way — see [`is_listed`].
pub async fn remove_from_favorites(state: &AppState, id: i64) -> Result<(), AppError> {
    set_favorite(state, id, false).await?;
    delete_if_unlisted(state, id).await
}

/// Drop a station out of the Recently Played tab.
///
/// The mirror of [`remove_from_favorites`]: forget the plays, and keep the row while a star still
/// lists it somewhere.
pub async fn remove_from_recent(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::radio::clear_play_history(&state.db, id).await?;
    delete_if_unlisted(state, id).await
}

/// Delete a row no tab would list any more.
///
/// The table backs the two local tabs and nothing else, so a station neither of them shows is a
/// row nothing can reach — including the user, who has just removed it from both.
///
/// **Every un-star owes this, not just the trash.** The star and the trash leave a station in the
/// same place; [`set_directory_favorite`] deliberately doesn't decide, so the surface calling it
/// has to, or a browse-and-unstar leaves a row behind on every pass.
pub async fn delete_if_unlisted(state: &AppState, id: i64) -> Result<(), AppError> {
    let station = get_station(state, id).await?;
    if is_listed(&station) {
        return Ok(());
    }
    queries::radio::delete_station(&state.db, id).await
}

/// Whether either local tab would still show a station: the star is Favorites' filter and the
/// stamp is Recently Played's, so between them they are the whole of what a row is kept for.
///
/// **A hand-typed station is listed by its star alone**, whatever it has been played. No directory
/// page names it, so the card offers no star to set (`starrable: station.uuid != ""`) and Browse
/// cannot write the row back. Counting the stamp there leaves it in Recently Played with the one
/// tab that could restore it unable to, which is the row-nothing-can-reach this predicate exists
/// to prevent rather than a milder version of it.
fn is_listed(station: &radio::RadioStation) -> bool {
    if station.station_uuid.as_deref().is_none_or(str::is_empty) {
        return station.is_favorite;
    }
    station.is_favorite || station.last_played.is_some()
}

#[cfg(test)]
#[path = "../tests/radio_tests.rs"]
mod tests;
