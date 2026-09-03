//! Favorite reads and writes, and the one place a favorite becomes a love.
//!
//! `set_favorite` and `toggle_current_favorite` are the two write entry points
//! and both end in `sync_love`, so provider arming, batching and the
//! best-effort policy are argued once rather than at each caller.
//! `backfill_loves` covers the library already favorited before the love toggle
//! was ever turned on.

use std::sync::Arc;

use crate::database::queries;
use crate::entities::{artist, track};
use crate::error::AppError;
use crate::player::state::{PlayerAction, lock_state, sync_current_track_if_in, with_state_emit};
use crate::services::scrobble::LoveTarget;
use crate::state::AppState;
use crate::utils::toast::{self, ToastKind};

pub async fn set_favorite(state: &AppState, ids: Vec<i64>, favorite: bool) -> Result<(), AppError> {
    queries::track::set_favorite(&state.db, &ids, favorite).await?;
    // After the write, so the line means it landed rather than was attempted.
    log::debug!("favorite: {} track(s) → {favorite}", ids.len());
    // If the currently-playing track was one of the toggled ids, mirror the new
    // flag onto `current_track` so the Now-Playing heart updates without waiting
    // for the next track load (parity with `toggle_current_favorite`).
    sync_current_track_favorite(state, &ids, favorite);
    state.library_changed.bump();
    sync_love(state, &ids, favorite).await;
    Ok(())
}

/// Mirror a favorite change to the scrobble services — Last.fm Loved Tracks plus
/// best-effort `ListenBrainz` feedback. Best-effort: a lookup or enqueue failure
/// is logged, never propagated, so it can't fail the favorite write. Skips all
/// DB work when love-sync is off or no provider is connected. Fetches the whole
/// id set in one bulk query and queues it under a single lock + save + wake
/// (via `enqueue_loves`), so a multi-select toggle is O(1) round-trips and disk
/// writes, not O(N).
async fn sync_love(state: &AppState, ids: &[i64], loved: bool) {
    if !state.scrobble.love_sync_active() {
        return;
    }
    let rows = match queries::track::get_scrobble_rows_by_ids(&state.db, ids).await {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("love-sync lookup failed for {} track(s): {e}", ids.len());
            return;
        }
    };
    if let Err(e) = state.scrobble.enqueue_loves(&rows, loved).await {
        log::warn!("love-sync enqueue failed: {e}");
    }
}

/// Retroactively push every existing favorite to `target`'s loved tracks — run
/// when a user turns on a provider's love toggle or connects that service while
/// its love toggle is already on, so they don't have to re-toggle each heart.
/// Best-effort and idempotent (loves coalesce and re-loving is a no-op on both
/// services); every failure is logged, never propagated. Reports the result as
/// an info toast. A no-op when `target` isn't armed or there are no favorites.
pub async fn backfill_loves(state: &AppState, target: LoveTarget) {
    if !state.scrobble.love_target_armed(target) {
        return;
    }
    let rows = match queries::track::get_favorite_scrobble_rows(&state.db).await {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("love backfill: favorites fetch failed: {e}");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    let total = rows.len();
    let queued = match state.scrobble.backfill_loves(&rows, target).await {
        Ok(n) => n,
        Err(e) => {
            log::warn!("love backfill: enqueue failed: {e}");
            return;
        }
    };
    let provider = match target {
        LoveTarget::Lastfm => "Last.fm",
        LoveTarget::ListenBrainz => "ListenBrainz",
    };
    log::info!("love backfill: queued {queued}/{total} favorites for {provider}");
    let detail = if queued > 0 {
        format!("Syncing {queued} loved track(s) to {provider}")
    } else if matches!(target, LoveTarget::ListenBrainz) {
        // Armed with favorites, but none carry a MusicBrainz ID for LB to key on.
        "No favorites have a MusicBrainz ID yet — turn on \"Add MusicBrainz IDs to your music\""
            .to_owned()
    } else {
        return;
    };
    toast::notify(ToastKind::LoveSync, detail);
}

/// If `current_track` is one of `ids`, flip its cached `is_favorite` and emit so
/// the Now-Playing surfaces reflect a favorite toggled from a list row.
fn sync_current_track_favorite(state: &AppState, ids: &[i64], favorite: bool) {
    sync_current_track_if_in(&state.player_state, &state.sinks, ids, |t| {
        t.is_favorite = favorite;
    });
}

/// Toggle the favorite flag on the currently playing track. Persists to DB,
/// flips `is_favorite` on `PlayerState.current_track` (so the next emit
/// rebuilds the view-model with the new value), and returns the affected
/// `(id, new_fav)` so callers can mirror the change into other UI surfaces
/// (e.g. the tracks list) without re-locking.
pub async fn toggle_current_favorite(state: &AppState) -> Result<Option<(i64, bool)>, AppError> {
    let Some((id, new_fav)) = ({
        let g = lock_state(&state.player_state);
        g.current_track().map(|t| (t.id, !t.is_favorite))
    }) else {
        return Ok(None);
    };

    queries::track::set_favorite(&state.db, &[id], new_fav).await?;
    log::debug!("favorite: playing track {id} → {new_fav}");

    with_state_emit(&state.player_state, &state.sinks, |s| {
        // Guard against a track change between the id read above and here: only
        // flip the cached flag if `current_track` is still the track we wrote.
        if let Some(track) = s.current_track_mut()
            && track.id == id
        {
            Arc::make_mut(track).is_favorite = new_fav;
        }
        Vec::<PlayerAction>::new()
    });

    state.library_changed.bump();

    sync_love(state, &[id], new_fav).await;

    Ok(Some((id, new_fav)))
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
