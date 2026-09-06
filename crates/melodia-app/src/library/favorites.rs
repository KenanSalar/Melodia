//! Favorite reads and writes, and the one place a favorite becomes a love.
//!
//! `set_favorite` and `toggle_current_favorite` are the two write entry points
//! and both end in `sync_love`, so provider arming, batching and the
//! best-effort policy are argued once rather than at each caller.
//! `backfill_loves` covers the library already favorited before the love toggle
//! was ever turned on.

use std::sync::Arc;

use crate::state::{AppState, Signal};
use melodia_core::entities::{artist, track};
use melodia_core::error::AppError;
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
        // After the write, so the line means it landed rather than was attempted.
        log::debug!("favorite: {} track(s) → {favorite}", ids.len());
        // If the currently-playing track was one of the toggled ids, mirror the new
        // flag onto `current_track` so the Now-Playing heart updates without waiting
        // for the next track load (parity with `toggle_current`).
        sync_current_track_if_in(&self.player_state, &self.sinks, ids, |t| {
            t.is_favorite = favorite;
        });
        self.library_changed.bump();
        self.sync_love(ids, favorite).await;
        Ok(())
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
            // Guard against a track change between the id read above and here: only
            // flip the cached flag if `current_track` is still the track we wrote.
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

    /// Mirror a favorite change to the scrobble services: Last.fm Loved Tracks plus
    /// best-effort `ListenBrainz` feedback. Best-effort, so a lookup or enqueue failure
    /// is logged, never propagated, and can't fail the favorite write. Skips all DB work
    /// when love-sync is off or no provider is connected. Fetches the whole id set in one
    /// bulk query and queues it under a single lock + save + wake (via `enqueue_loves`),
    /// so a multi-select toggle is O(1) round-trips and disk writes, not O(N).
    async fn sync_love(&self, ids: &[i64], loved: bool) {
        if !self.scrobble.love_sync_active() {
            return;
        }
        let rows = match queries::track::get_scrobble_rows_by_ids(&self.db, ids).await {
            Ok(rows) => rows,
            Err(e) => {
                log::warn!("love-sync lookup failed for {} track(s): {e}", ids.len());
                return;
            }
        };
        if let Err(e) = self.scrobble.enqueue_loves(&rows, loved).await {
            log::warn!("love-sync enqueue failed: {e}");
        }
    }
}

pub async fn set_favorite(state: &AppState, ids: Vec<i64>, favorite: bool) -> Result<(), AppError> {
    FavoriteWrite::from_state(state).set(&ids, favorite).await
}

/// Toggle the favorite flag on the currently playing track. Persists to DB,
/// flips `is_favorite` on `PlayerState.current_track` (so the next emit
/// rebuilds the view-model with the new value), and returns the affected
/// `(id, new_fav)` so callers can mirror the change into other UI surfaces
/// (e.g. the tracks list) without re-locking.
pub async fn toggle_current_favorite(state: &AppState) -> Result<Option<(i64, bool)>, AppError> {
    FavoriteWrite::from_state(state).toggle_current().await
}

/// Retroactively push every existing favorite to `target`'s loved tracks — run
/// when a user turns on a provider's love toggle or connects that service while
/// its love toggle is already on, so they don't have to re-toggle each heart.
/// Best-effort and idempotent (loves coalesce and re-loving is a no-op on both
/// services); every failure is logged, never propagated. Reports the result as
/// an info toast. A no-op when `target` isn't armed or there are no favorites.
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
            log::warn!("love backfill: favorites fetch failed: {e}");
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
            log::warn!("love backfill: enqueue failed: {e}");
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

pub async fn get_favorite_tracks(state: &AppState) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_favorite_tracks_for_list(&state.db).await
}

pub async fn get_favorite_stats(state: &AppState) -> Result<track::FavoriteStats, AppError> {
    queries::track::get_favorite_stats(&state.db).await
}

pub async fn get_favorite_artists(
    state: &AppState,
) -> Result<Vec<artist::FavoriteArtist>, AppError> {
    queries::artist::get_favorite_artists(&state.db).await
}

/// Favorite tracks ranked by play count — the whole set, since the Most Played
/// tab is a virtualized grid and has no reason to truncate.
pub async fn get_most_played_favorites(
    state: &AppState,
) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    queries::track::get_most_played_favorites(&state.db).await
}

#[cfg(test)]
#[path = "tests/favorites_tests.rs"]
mod tests;
