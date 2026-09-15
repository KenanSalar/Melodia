//! Smart (dynamic) playlist orchestration — thin async wrappers over the
//! [`queries::smart_playlist`] evaluator and the smart-playlist CRUD in
//! [`queries::playlist`]. Membership is resolved live from the stored criteria;
//! nothing is ever materialized into `playlist_items`.

use crate::state::AppState;
use melodia_core::entities::playlist::{Playlist, PlaylistStats};
use melodia_core::entities::smart_criteria::SmartCriteria;
use melodia_core::entities::track::TrackListRow;
use melodia_core::error::{AppError, describe};
use melodia_store::database::queries;

/// Resolve a smart playlist's current membership from its criteria (ordered and
/// capped per the criteria's optional limit).
pub async fn evaluate(
    state: &AppState,
    criteria: &SmartCriteria,
) -> Result<Vec<TrackListRow>, AppError> {
    queries::smart_playlist::get_smart_playlist_tracks(&state.db, criteria).await
}

/// `(track_count, total_duration_ms)` for a smart playlist, whose stats can't come from the
/// `playlist_items` triggers.
pub async fn count(state: &AppState, criteria: &SmartCriteria) -> Result<(i64, i64), AppError> {
    queries::smart_playlist::count_smart_playlist(&state.db, criteria).await
}

/// Replaces the stats of the smart playlists at `rows` with what their rules match now, the
/// stored ones being the junction triggers' and so always 0. A row whose count fails keeps its
/// stats and is logged.
pub async fn recount(state: &AppState, playlists: &mut [PlaylistStats], rows: &[usize]) {
    // WAL lets these run side by side across the read pool.
    let parsed: Vec<(usize, SmartCriteria)> = rows
        .iter()
        .map(|&i| (i, SmartCriteria::from_json_opt(playlists[i].smart_criteria.as_deref())))
        .collect();
    let counts = futures_util::future::join_all(
        parsed.iter().map(|(i, criteria)| async move { (*i, count(state, criteria).await) }),
    )
    .await;
    for (i, result) in counts {
        match result {
            Ok((track_count, duration_ms)) => {
                playlists[i].track_count = i32::try_from(track_count).unwrap_or(i32::MAX);
                playlists[i].total_duration_ms = duration_ms;
            }
            Err(e) => {
                log::warn!("smart playlist {} count failed: {}", playlists[i].id, describe(&e));
            }
        }
    }
}

/// Serialize a rule set to the JSON stored in `playlists.smart_criteria`,
/// mapping a serializer failure to a validation error.
fn criteria_to_json(criteria: &SmartCriteria) -> Result<String, AppError> {
    criteria.to_json().map_err(|e| AppError::Validation(format!("serialize smart_criteria: {e}")))
}

/// Persist a new smart playlist and bump `library_changed` so the grid
/// refreshes (same signal every playlist CRUD uses).
pub async fn create_smart_playlist(
    state: &AppState,
    name: String,
    description: Option<String>,
    criteria: &SmartCriteria,
) -> Result<Playlist, AppError> {
    let criteria_json = criteria_to_json(criteria)?;
    let playlist = queries::playlist::create_smart_playlist(
        &state.db,
        &name,
        description.as_deref(),
        &criteria_json,
    )
    .await?;
    state.library_changed.bump();
    Ok(playlist)
}

/// Replace a smart playlist's rule set and bump `library_changed`.
pub async fn update_smart_criteria(
    state: &AppState,
    id: i64,
    criteria: &SmartCriteria,
) -> Result<Playlist, AppError> {
    let criteria_json = criteria_to_json(criteria)?;
    let playlist = queries::playlist::update_smart_criteria(&state.db, id, &criteria_json).await?;
    state.library_changed.bump();
    Ok(playlist)
}
