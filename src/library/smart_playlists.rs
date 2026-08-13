//! Smart (dynamic) playlist orchestration — thin async wrappers over the
//! [`queries::smart_playlist`] evaluator and the smart-playlist CRUD in
//! [`queries::playlist`]. Membership is resolved live from the stored criteria;
//! nothing is ever materialized into `playlist_items`.

use crate::database::queries;
use crate::entities::playlist::Playlist;
use crate::entities::smart_criteria::SmartCriteria;
use crate::entities::track::TrackListRow;
use crate::error::AppError;
use crate::state::AppState;

/// Resolve a smart playlist's current membership from its criteria (ordered and
/// capped per the criteria's optional limit).
pub async fn evaluate(
    state: &AppState,
    criteria: &SmartCriteria,
) -> Result<Vec<TrackListRow>, AppError> {
    queries::smart_playlist::get_smart_playlist_tracks(&state.db, criteria).await
}

/// `(track_count, total_duration_ms)` for a smart playlist — used by the grid
/// card, whose stats can't come from the `playlist_items` triggers.
pub async fn count(state: &AppState, criteria: &SmartCriteria) -> Result<(i64, i64), AppError> {
    queries::smart_playlist::count_smart_playlist(&state.db, criteria).await
}

/// Serialize a rule set to the JSON stored in `playlists.smart_criteria`,
/// mapping a serializer failure to a validation error.
fn criteria_to_json(criteria: &SmartCriteria) -> Result<String, AppError> {
    criteria
        .to_json()
        .map_err(|e| AppError::Validation(format!("serialize smart_criteria: {e}")))
}

/// Persist a new smart playlist and bump `library_changed_tx` so the grid
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
    state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));
    Ok(playlist)
}

/// Replace a smart playlist's rule set and bump `library_changed_tx`.
pub async fn update_smart_criteria(
    state: &AppState,
    id: i64,
    criteria: &SmartCriteria,
) -> Result<Playlist, AppError> {
    let criteria_json = criteria_to_json(criteria)?;
    let playlist = queries::playlist::update_smart_criteria(&state.db, id, &criteria_json).await?;
    state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));
    Ok(playlist)
}
