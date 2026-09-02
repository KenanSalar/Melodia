use std::sync::Arc;

use crate::database::queries;
use crate::error::AppError;
use crate::media::rating_tags;
use crate::media::tag_writer::{FieldEdit, TagEdit};
use crate::player::state::{PlayerAction, lock_state, sync_current_track_if_in, with_state_emit};
use crate::state::AppState;
use crate::tasks::rating_writeback;

/// Star ratings live in the range 0–5 (0 = unrated). Both public setters clamp
/// through here so a hand-edited or out-of-range value can never reach the DB.
///
/// The bound is [`rating_tags`]'s, not a second copy of it — the same number caps what
/// reaches a file, and two spellings of one range is exactly what drifts.
fn clamp_rating(rating: i32) -> i32 {
    rating_tags::clamp_stars(rating)
}

/// Set the star rating (0–5) on one or more tracks by ID. Mirrors
/// [`crate::library::favorites::set_favorite`]: persist to DB, then bump
/// `library_changed` so the visibility-gated list views re-fetch. Rating is
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
    rating_writeback::enqueue(&ids, rating);
    state.library_changed.bump();
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
    rating_writeback::enqueue(&[id], rating);

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

    state.library_changed.bump();

    Ok(Some((id, rating)))
}

/// Write `rating` into each track's own file, so the star outlives this database.
///
/// Goes through [`crate::library::tags::write_tag_edit`] — the tag-edit core, minus the parts a
/// rating doesn't need. The `SelfWrites` mark, the re-extract and the `update_track_metadata`
/// that keeps `file_hash` / `file_size` / `date_modified` honest all come with it; what is
/// deliberately left behind is [`crate::library::tags::apply_tag_edit`]'s wrapper, whose
/// `library_changed` bump would make every open list re-fetch for a value they are already
/// showing, and whose player resync answers to fields a rating write cannot change.
///
/// Failures are the caller's to log: a file can be read-only, or a container can have no tag to
/// hold a rating, and neither is a reason to undo a star the user set.
pub(crate) async fn write_rating_to_files(
    state: &AppState,
    ids: &[i64],
    rating: i32,
) -> Result<usize, AppError> {
    let edit = TagEdit {
        rating: FieldEdit::Set(clamp_rating(rating)),
        ..TagEdit::default()
    };
    let (report, _) = super::tags::write_tag_edit(
        &state.db,
        &state.paths.artwork_dir,
        &state.cover_cache,
        &state.self_writes,
        ids,
        &edit,
        None,
    )
    .await?;

    for (file, err) in &report.failures {
        log::warn!("rating: {file} kept its row but not its tag: {err}");
    }
    Ok(report.updated)
}

#[cfg(test)]
#[path = "tests/ratings_tests.rs"]
mod tests;
