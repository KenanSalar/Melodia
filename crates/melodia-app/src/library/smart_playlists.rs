//! Smart playlists, whose membership resolves live from the stored criteria and is never
//! materialized into `playlist_items`.

use crate::state::AppState;
use melodia_core::entities::playlist::{Playlist, PlaylistStats};
use melodia_core::entities::smart_criteria::{Draw, SmartCriteria};
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::{AppError, describe};
use melodia_store::database::{DbPool, queries};

/// `(track_count, total_duration_ms)` for a smart playlist, whose stats can't come from the
/// `playlist_items` triggers.
pub async fn count(db: &DbPool, criteria: &SmartCriteria) -> Result<(i64, i64), AppError> {
    queries::smart_playlist::count_smart_playlist(db, criteria).await
}

/// Fills in what the smart rows among `playlists` take from their rules: the stats, which the
/// junction triggers leave at 0, and the automatic thumbnail, which the stored row never holds.
/// Manual rows are left as read.
pub async fn fill_from_rules(db: &DbPool, playlists: &mut [PlaylistStats]) {
    // WAL lets these run side by side across the read pool.
    futures_util::future::join_all(playlists.iter_mut().filter(|p| p.is_smart).map(
        |playlist| async move {
            recount(db, playlist).await;
            fill_automatic_thumbnail(db, playlist).await;
        },
    ))
    .await;
}

/// A row whose count fails keeps its stats and is logged.
async fn recount(db: &DbPool, playlist: &mut PlaylistStats) {
    let criteria = SmartCriteria::from_json_opt(playlist.smart_criteria.as_deref());
    match count(db, &criteria).await {
        Ok((track_count, duration_ms)) => {
            playlist.track_count = i32::try_from(track_count).unwrap_or(i32::MAX);
            playlist.total_duration_ms = duration_ms;
        }
        Err(e) => log::warn!("smart playlist {} count failed: {}", playlist.id, describe(&e)),
    }
}

/// Points a smart playlist's thumbnail at the first cover its rules reach, as a manual playlist's
/// automatic one is its first track's. Worked out on every read rather than stored, the membership
/// it follows being live. A custom pick is left alone, and so is a Clear, which pins the
/// placeholder.
///
/// Under the default draw, so a random cap's card and hero agree on one cover across opens. A
/// failed lookup keeps the stored one and is logged.
pub async fn fill_automatic_thumbnail(db: &DbPool, playlist: &mut PlaylistStats) {
    if !playlist.is_smart || playlist.custom_thumbnail {
        return;
    }
    let scope = TrackListScope::Playlist(playlist.id, Draw::default());
    match queries::track::first_cover(db, scope).await {
        Ok(cover) => playlist.thumbnail_path = cover,
        Err(e) => log::warn!("smart playlist {} cover failed: {}", playlist.id, describe(&e)),
    }
}

/// Which of `ids` are smart playlists whose rules read play stats, the ones a play-count flush
/// can move. A static rule ("Genre is Rock") can't, so a flush leaves it uncounted.
pub async fn reading_play_stats(db: &DbPool, ids: &[i64]) -> Result<Vec<i64>, AppError> {
    let rows = queries::playlist::smart_criteria_for_playlists(db, ids).await?;
    Ok(rows
        .into_iter()
        .filter(|(_, is_smart, criteria)| {
            *is_smart && SmartCriteria::from_json_opt(criteria.as_deref()).depends_on_play_stats()
        })
        .map(|(id, _, _)| id)
        .collect())
}

/// Serialize a rule set to the JSON stored in `playlists.smart_criteria`.
fn criteria_to_json(criteria: &SmartCriteria) -> Result<String, AppError> {
    criteria.to_json().map_err(|e| AppError::io("serialize smart_criteria", e))
}

/// Persist a new smart playlist and bump `library_changed`.
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

#[cfg(test)]
#[path = "tests/smart_playlists_tests.rs"]
mod tests;
