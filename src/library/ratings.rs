use std::sync::Arc;

use crate::database::queries;
use crate::error::AppError;
use crate::player::state::{PlayerAction, lock_state, sync_current_track_if_in, with_state_emit};
use crate::state::AppState;

/// Star ratings live in the range 0–5 (0 = unrated). Both public setters clamp
/// through here so a hand-edited or out-of-range value can never reach the DB.
fn clamp_rating(rating: i32) -> i32 {
    rating.clamp(0, 5)
}

/// Set the star rating (0–5) on one or more tracks by ID. Mirrors
/// [`crate::library::favorites::set_favorite`]: persist to DB, then bump
/// `library_changed_tx` so the visibility-gated list views re-fetch. Rating is
/// orthogonal to list membership (unlike un-favoriting), so callers pair this
/// with an optimistic per-row `VecModel` patch for instant feedback.
pub async fn set_rating(state: &AppState, ids: Vec<i64>, rating: i32) -> Result<(), AppError> {
    let rating = clamp_rating(rating);
    queries::track::set_rating(&state.db, &ids, rating).await?;
    // After the write, so the line means it landed rather than was attempted.
    log::debug!("rating: {} track(s) → {rating}", ids.len());
    // If the currently-playing track was one of the rated ids, mirror the new
    // rating onto `current_track` so the Now-Playing star strip updates without
    // waiting for the next track load (parity with `set_current_rating`).
    sync_current_track_rating(state, &ids, rating);
    state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));
    Ok(())
}

/// If `current_track` is one of `ids`, flip its cached `rating` and emit so the
/// Now-Playing surfaces reflect a rating set from a list row.
fn sync_current_track_rating(state: &AppState, ids: &[i64], rating: i32) {
    sync_current_track_if_in(&state.player_state, &state.sinks, ids, |t| t.rating = rating);
}

/// Set the star rating on the currently playing track. Persists to DB, flips
/// `rating` on `PlayerState.current_track` (so the next emit rebuilds the
/// view-model with the new value), and returns the affected `(id, rating)` so
/// callers can mirror the change into the other UI surfaces without re-locking.
/// Mirrors [`crate::library::favorites::toggle_current_favorite`].
pub async fn set_current_rating(
    state: &AppState,
    rating: i32,
) -> Result<Option<(i64, i32)>, AppError> {
    let rating = clamp_rating(rating);
    let Some(id) = ({
        let g = lock_state(&state.player_state);
        g.current_track().map(|t| t.id)
    }) else {
        return Ok(None);
    };

    queries::track::set_rating(&state.db, &[id], rating).await?;
    log::debug!("rating: playing track {id} → {rating}");

    with_state_emit(&state.player_state, &state.sinks, |s| {
        // Guard against a track change between the id read above and here: only
        // flip the cached rating if `current_track` is still the track we wrote.
        if let Some(track) = s.current_track_mut()
            && track.id == id
        {
            Arc::make_mut(track).rating = rating;
        }
        Vec::<PlayerAction>::new()
    });

    state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));

    Ok(Some((id, rating)))
}

#[cfg(test)]
#[path = "tests/ratings_tests.rs"]
mod tests;
