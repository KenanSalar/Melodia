//! Favorite reads and writes, and the one place a favorite becomes a love: every write entry point
//! ends in `sync_love`, so provider arming, batching and the best-effort policy are argued once.
//! `backfill_loves` covers what was favorited before the love toggle was on.

use std::sync::Arc;

use crate::state::{AppState, Signal};
use melodia_core::entities::track;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::toast::{self, ToastKind};
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::{
    PlayerAction, PlayerStateHandle, lock_state, sync_current_track_if_in, with_state_emit,
};
use melodia_integrations::services::integrations::scrobble::{LoveTarget, ScrobbleService};
use melodia_store::database::{DbPool, queries};

/// What a favorite write reaches: the row, the cached copy the Now-Playing surfaces
/// read, the subscribers that re-fetch off it, and the provider mirror.
///
/// Owned clones rather than an `&AppState`, for `tasks::rating_writeback::Writeback`'s
/// reason. Every field is cheap to clone, and naming them is what lets the order these
/// two writers share be driven by a test at all.
struct FavoriteWrite {
    db: DbPool,
    player_state: Arc<PlayerStateHandle>,
    sinks: Arc<PlayerSinks>,
    library_changed: Signal,
    scrobble: Arc<ScrobbleService>,
}

impl FavoriteWrite {
    fn from_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
            player_state: state.player_state.clone(),
            sinks: state.sinks.clone(),
            library_changed: state.library_changed.clone(),
            scrobble: state.scrobble.clone(),
        }
    }

    async fn set(&self, ids: &[i64], favorite: bool) -> Result<(), AppError> {
        queries::track::set_favorite(&self.db, ids, favorite).await?;
        self.propagate(ids, favorite).await;
        Ok(())
    }

    /// Answers the ids it un-hearted, an empty answer having written nothing anywhere.
    async fn unfavorite(&self, ids: &[i64]) -> Result<Vec<i64>, AppError> {
        let cleared = queries::track::unfavorite(&self.db, ids).await?;
        if !cleared.is_empty() {
            self.propagate(&cleared, false).await;
        }
        Ok(cleared)
    }

    /// What follows a write that landed: the playing copy, the subscribers, the provider mirror.
    async fn propagate(&self, ids: &[i64], favorite: bool) {
        // After the write, so the line means it landed rather than was attempted.
        log::debug!("favorite: {} track(s) → {favorite}", ids.len());
        sync_current_track_if_in(&self.player_state, &self.sinks, ids, |t| {
            t.is_favorite = favorite;
        });
        self.library_changed.bump();
        self.sync_love(ids, favorite).await;
    }

    async fn toggle_current(&self) -> Result<Option<(i64, bool)>, AppError> {
        let Some((id, new_fav)) = ({
            let g = lock_state(&self.player_state);
            g.current_track().map(|t| (t.id, !t.is_favorite))
        }) else {
            return Ok(None);
        };

        queries::track::set_favorite(&self.db, &[id], new_fav).await?;
        log::debug!("favorite: playing track {id} → {new_fav}");

        with_state_emit(&self.player_state, &self.sinks, |s| {
            // The track can have changed since the id was read.
            if let Some(track) = s.current_track_mut()
                && track.id == id
            {
                Arc::make_mut(track).is_favorite = new_fav;
            }
            Vec::<PlayerAction>::new()
        });

        self.library_changed.bump();

        self.sync_love(&[id], new_fav).await;

        Ok(Some((id, new_fav)))
    }

    /// Mirror a favorite change to the scrobble services, best-effort: a failure is logged and
    /// can't fail the favorite write. One bulk fetch and one enqueue for the whole id set, so a
    /// multi-select toggle costs one round trip and one save.
    async fn sync_love(&self, ids: &[i64], loved: bool) {
        if !self.scrobble.love_sync_active() {
            return;
        }
        let rows = match queries::track::get_scrobble_rows_by_ids(&self.db, ids).await {
            Ok(rows) => rows,
            Err(e) => {
                log::warn!("love-sync lookup failed for {} track(s): {}", ids.len(), describe(&e));
                return;
            }
        };
        if let Err(e) = self.scrobble.enqueue_loves(&rows, loved).await {
            log::warn!("love-sync enqueue failed: {}", describe(&e));
        }
    }
}

pub async fn set_favorite(state: &AppState, ids: Vec<i64>, favorite: bool) -> Result<(), AppError> {
    FavoriteWrite::from_state(state).set(&ids, favorite).await
}

/// Takes the heart off those of `ids` that carry one, answering exactly those for an undo to
/// re-heart through [`set_favorite`].
pub async fn unfavorite(state: &AppState, ids: Vec<i64>) -> Result<Vec<i64>, AppError> {
    FavoriteWrite::from_state(state).unfavorite(&ids).await
}

/// Toggle the playing track's favorite, answering `(id, new_fav)` so callers can mirror it into
/// the other surfaces without re-locking.
pub async fn toggle_current_favorite(state: &AppState) -> Result<Option<(i64, bool)>, AppError> {
    FavoriteWrite::from_state(state).toggle_current().await
}

/// Push every existing favorite to `target`'s loved tracks, for a love toggle turned on or a
/// service connected with it on, so nobody re-toggles each heart. Best-effort and idempotent,
/// re-loving being a no-op on both services.
pub async fn backfill_loves(state: &AppState, target: LoveTarget) {
    let Some(queued) = queue_favorite_loves(&state.db, &state.scrobble, target).await else {
        return;
    };
    if let Some(detail) = backfill_detail(queued, target) {
        toast::notify(ToastKind::LoveSync, detail);
    }
}

/// [`backfill_loves`]'s body, narrowed to what it reaches. `None` where nothing was
/// attempted (target unarmed, no favorites, or either call failed), so the door reports
/// only on a pass that actually ran.
async fn queue_favorite_loves(
    db: &DbPool,
    scrobble: &ScrobbleService,
    target: LoveTarget,
) -> Option<usize> {
    if !scrobble.love_target_armed(target) {
        return None;
    }
    let rows = match queries::track::get_favorite_scrobble_rows(db).await {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("love backfill: favorites fetch failed: {}", describe(&e));
            return None;
        }
    };
    if rows.is_empty() {
        return None;
    }
    let total = rows.len();
    let queued = match scrobble.backfill_loves(&rows, target).await {
        Ok(n) => n,
        Err(e) => {
            log::warn!("love backfill: enqueue failed: {}", describe(&e));
            return None;
        }
    };
    log::info!("love backfill: queued {queued}/{total} favorites for {}", provider_name(target));
    Some(queued)
}

/// What the backfill tells the user, or `None` to stay quiet. Last.fm takes every
/// favorite it was handed, so a zero there means the toggle moved out from under the
/// fetch and there is nothing to report. A `ListenBrainz` zero is an answer: none of the
/// favorites carries the `MusicBrainz` ID its feedback keys on.
fn backfill_detail(queued: usize, target: LoveTarget) -> Option<String> {
    if queued > 0 {
        return Some(format!("Syncing {queued} loved track(s) to {}", provider_name(target)));
    }
    matches!(target, LoveTarget::ListenBrainz).then(|| {
        "No favorites have a MusicBrainz ID yet — turn on \"Add MusicBrainz IDs to your music\""
            .to_owned()
    })
}

/// The provider's own spelling, shared by the backfill's log line and its toast.
fn provider_name(target: LoveTarget) -> &'static str {
    match target {
        LoveTarget::Lastfm => "Last.fm",
        LoveTarget::ListenBrainz => "ListenBrainz",
    }
}

pub async fn get_favorite_stats(db: &DbPool) -> Result<track::FavoriteStats, AppError> {
    queries::track::get_favorite_stats(db).await
}

#[cfg(test)]
#[path = "tests/favorites_tests.rs"]
mod tests;
