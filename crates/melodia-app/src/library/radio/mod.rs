//! Radio library API, and the only door the UI has onto stations.
//!
//! The stored table and the radio-browser.info directory answer through one module, so a callback
//! never learns which did and the off switch has one place to guard
//! (`docs/adr/0015-radio-ships-off-guarded-at-the-facade.md`). The door is the module rather than
//! this file: every submodule is re-exported here, and what stays is what all of them need.
//!
//! **`ensure_enabled` and `directory_client` may not move.** A submodule reaching
//! `state.http_client()` directly is traffic a user who turned Radio off still pays;
//! `crates/melodia/tests/radio_facade.rs` walks every file under this directory for it.
//!
//! Writes here don't bump `library_changed`: none of its subscribers shows a station.

mod authoring;
mod directory;
mod logos;
mod playback;
mod removal;

pub use authoring::{add_custom_station, set_station_overrides, update_custom_station};
pub use directory::{facets, search, station_details, vote};
pub use logos::{
    AnswerSeed, LogoAnswer, SiteOrigin, artwork_is_present, classify_logo_answer,
    discover_site_logo, fetch_logo, heal_seed_urls, heal_station_logo, logo_answers,
    prune_logo_answers, record_logo_outcome, set_artwork, site_origin,
};
pub use playback::{
    mark_played, play_directory_station, play_station, record_probed_codec, set_directory_favorite,
    station_to_restore,
};
pub use removal::{RemovedStations, delete_if_unlisted, remove_stations, restore_stations};

// `library::radio_files` composes these with its own write, argued at each definition.
pub(super) use authoring::{resolve_station_name, validated_overrides};

use crate::state::AppState;
use melodia_core::entities::radio;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// How far back the recently-played station list reaches. It is a way back to what you were just
/// listening to rather than a record, so it is sized to a session or two of tuning around.
pub const RECENT_STATIONS_LIMIT: i64 = 50;

/// A refusal when the user has switched Radio off: **this is where "off" means no traffic**, the
/// sidebar row being something a stale callback is already past. The getters below stay open,
/// hiding a feature not being deleting what the user kept.
pub(super) fn ensure_enabled(state: &AppState) -> Result<(), AppError> {
    if state.radio_enabled.get() {
        return Ok(());
    }
    Err(AppError::settings_msg("Radio is switched off"))
}

/// The shared client, reachable only past [`ensure_enabled`]. The logo download takes it too, the
/// guard being about traffic rather than about who answers.
pub(super) fn directory_client(state: &AppState) -> Result<&reqwest::Client, AppError> {
    ensure_enabled(state)?;
    Ok(state.http_client())
}

/// Every favorited station, naturally name-ordered.
pub async fn get_favorites(db: &DbPool) -> Result<Vec<radio::RadioStation>, AppError> {
    queries::radio::get_favorite_stations(db).await
}

/// The stations played most recently, newest first.
pub async fn get_recent(db: &DbPool) -> Result<Vec<radio::RadioStation>, AppError> {
    queries::radio::get_recent_stations(db, RECENT_STATIONS_LIMIT).await
}

/// One station, or `AppError::NotFound` if it is gone.
pub async fn get_station(db: &DbPool, id: i64) -> Result<radio::RadioStation, AppError> {
    queries::radio::get_station_by_id(db, id).await
}

/// Persist a station, updating the row the directory already knows, and answer its id. Keeps
/// everything the user did with it.
pub async fn save_station(db: &DbPool, station: &radio::NewRadioStation) -> Result<i64, AppError> {
    queries::radio::save_station(db, station).await
}

pub async fn set_favorite(db: &DbPool, id: i64, favorite: bool) -> Result<(), AppError> {
    queries::radio::set_favorite(db, id, favorite).await
}

#[cfg(test)]
#[path = "../tests/radio_tests.rs"]
mod tests;
