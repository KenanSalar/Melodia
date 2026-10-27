use std::path::Path;
use std::sync::Arc;

use crate::state::AppState;
use crate::tasks::rating_writeback;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::entities::tags::{FieldEdit, TagEdit};
use melodia_core::error::AppError;
use melodia_core::utils::self_writes::SelfWrites;
use melodia_engine::player::engine::state::{
    PlayerAction, lock_state, sync_current_track_if_in, with_state_emit,
};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::media::ingest::rating_tags;

/// 0 is unrated. The bound is [`rating_tags`]'s, so what reaches the database and what reaches a
/// file can't drift apart.
fn clamp_rating(rating: i32) -> i32 {
    rating_tags::clamp_stars(rating)
}

/// Rating never changes list membership, so callers pair this with an optimistic per-row patch.
pub async fn set_rating(state: &AppState, ids: Vec<i64>, rating: i32) -> Result<(), AppError> {
    let rating = clamp_rating(rating);
    queries::track::set_rating(&state.db, &ids, rating).await?;
    // After the write, so the line means it landed rather than was attempted.
    log::debug!("rating: {} track(s) → {rating}", ids.len());
    sync_current_track_rating(state, &ids, rating);
    rating_writeback::enqueue(&ids, rating);
    state.library_changed.bump();
    Ok(())
}

/// Mirrors the rating onto `current_track` when it is one of `ids`, so Now Playing reflects a rating
/// set from a list row.
fn sync_current_track_rating(state: &AppState, ids: &[i64], rating: i32) {
    sync_current_track_if_in(&state.player_state, &state.sinks, ids, |t| t.rating = rating);
}

/// Rate the playing track, answering the `(id, rating)` written so callers can mirror it into the
/// other surfaces without re-locking.
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
        // The track can have changed since the id was read.
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
/// Through [`crate::library::tags::write_tag_edit`], which brings the `SelfWrites` mark and the
/// re-extract that keeps hash, size and mtime honest, rather than `apply_tag_edit`, whose
/// `library_changed` bump would re-fetch every open list for a value already shown. Failures are
/// the caller's to log: a read-only file or a container with no rating tag is no reason to undo a
/// star.
pub(crate) async fn write_rating_to_files(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &Arc<SelfWrites>,
    ids: &[i64],
    rating: i32,
) -> Result<usize, AppError> {
    let edit = TagEdit { rating: FieldEdit::Set(clamp_rating(rating)), ..TagEdit::default() };
    let (report, _) =
        super::tags::write_tag_edit(db, artwork_dir, cover_cache, self_writes, ids, &edit, None)
            .await?;

    for (file, reason) in &report.failures {
        log::warn!("rating: {file} kept its row but not its tag: {reason}");
    }
    Ok(report.updated)
}

#[cfg(test)]
#[path = "tests/ratings_tests.rs"]
mod tests;
