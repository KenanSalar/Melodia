use std::sync::Arc;

use crate::database::queries;
use crate::entities::track::TrackSummary;
use crate::error::AppError;
use crate::player::actions::execute_actions;
use crate::player::state::{lock_state, play_track_inner, with_state_emit};
use crate::player::types::PlaybackStatus;
use crate::state::PlaybackContext;

pub async fn player_play_track(ctx: &PlaybackContext, track_id: i64) -> Result<(), AppError> {
    let summary = queries::track::get_track_summary_by_id(&ctx.db, track_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("track {track_id}")))?;
    let summary = Arc::new(summary);

    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.queue.set_direct_play(Arc::clone(&summary));
        play_track_inner(s, summary, None)
    });

    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub async fn player_play_tracks(
    ctx: &PlaybackContext,
    track_ids: Vec<i64>,
    start_index: Option<usize>,
) -> Result<(), AppError> {
    let summaries: Vec<Arc<TrackSummary>> =
        queries::track::get_track_summaries_by_ids(&ctx.db, &track_ids)
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();

    if summaries.is_empty() {
        return Err(AppError::Queue("No valid tracks provided".to_owned()));
    }

    let start = start_index.unwrap_or(0).min(summaries.len() - 1);
    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.queue.clear();
        s.queue.add_tracks(summaries);
        s.queue.current_index = Some(start);
        s.queue.clear_direct_play();

        if let Some(track) = s.queue.get_current().cloned() {
            play_track_inner(s, track, None)
        } else {
            vec![]
        }
    });

    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_play(ctx: &PlaybackContext) -> Result<(), AppError> {
    let actions = with_state_emit(
        &ctx.player_state,
        &ctx.sinks,
        crate::player::state::PlayerState::build_play_actions,
    );
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_pause(ctx: &PlaybackContext) -> Result<(), AppError> {
    let actions = with_state_emit(
        &ctx.player_state,
        &ctx.sinks,
        crate::player::state::PlayerState::build_pause_actions,
    );
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

/// User-initiated stop — preserves `current_track` and `position_ms` so `player_play` can resume
/// from where the user stopped. Contrast with `stop_end_of_queue` which resets position to 0.
/// Toggle play/pause based on current playback status. Branching happens inside
/// the state lock so two near-simultaneous toggles (e.g. UI + media-key) can't
/// race past each other.
pub fn player_toggle_play_pause(ctx: &PlaybackContext) -> Result<(), AppError> {
    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| match s.status {
        PlaybackStatus::Playing | PlaybackStatus::Loading => s.build_pause_actions(),
        PlaybackStatus::Paused | PlaybackStatus::Stopped => s.build_play_actions(),
    });
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_stop(ctx: &PlaybackContext) -> Result<(), AppError> {
    let actions = with_state_emit(
        &ctx.player_state,
        &ctx.sinks,
        crate::player::state::PlayerState::build_stop_actions,
    );
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_seek(ctx: &PlaybackContext, position_ms: u64) -> Result<(), AppError> {
    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.build_seek_actions(position_ms)
    });
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_next(ctx: &PlaybackContext) -> Result<(), AppError> {
    let actions = with_state_emit(
        &ctx.player_state,
        &ctx.sinks,
        crate::player::state::PlayerState::build_next_actions,
    );
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_previous(ctx: &PlaybackContext) -> Result<(), AppError> {
    let actions = with_state_emit(
        &ctx.player_state,
        &ctx.sinks,
        crate::player::state::PlayerState::build_previous_actions,
    );
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_set_volume(ctx: &PlaybackContext, level: u32) -> Result<(), AppError> {
    // Fast no-op short-circuit: skip the whole with_state_emit / rodio /
    // ViewModel-watch dance when nothing would actually change. Catches
    // duplicate slider emits at the boundary.
    {
        let s = lock_state(&ctx.player_state);
        if s.volume == level && !s.is_muted {
            return Ok(());
        }
    }
    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.build_set_volume_actions(level)
    });
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    // Persistence is intentionally NOT triggered here. The slider fires
    // `set_volume` continuously during drag for live audio; settings.json
    // is flushed once via `commit_player_settings` on slider release.
    Ok(())
}

pub async fn player_set_muted(ctx: &PlaybackContext, muted: bool) -> Result<(), AppError> {
    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.build_set_muted_actions(muted)
    });
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    // Mute is single-tap; commit inline.
    commit_player_settings(ctx).await
}

pub async fn player_toggle_mute(ctx: &PlaybackContext) -> Result<(), AppError> {
    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.build_toggle_mute_actions()
    });
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    commit_player_settings(ctx).await
}

/// Persist the current `PlayerState`'s volume + `is_muted` into settings.json.
/// Called once from the volume slider's `pointer-event Up` (after a drag
/// or click), and inline from the mute mutators / souvlaki `SetVolume`.
///
/// Reads-then-writes settings.json on a `spawn_blocking` thread so the
/// async runtime worker isn't blocked. Short-circuits when settings already
/// match (common — e.g. a click that didn't change the value).
pub async fn commit_player_settings(ctx: &PlaybackContext) -> Result<(), AppError> {
    let (volume, is_muted) = {
        let s = lock_state(&ctx.player_state);
        (s.volume, s.is_muted)
    };
    let paths = ctx.paths.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let mut s = crate::services::settings::read_settings(&paths)?;
        if s.volume == volume && s.playback.is_muted == is_muted {
            return Ok(());
        }
        s.volume = volume;
        s.playback.is_muted = is_muted;
        crate::services::settings::write_settings(&paths, &s)
    })
    .await
    .map_err(|e| AppError::Settings(format!("commit_player_settings join: {e}")))?
}

pub fn player_set_playback_speed(ctx: &PlaybackContext, speed: f64) -> Result<(), AppError> {
    let actions = with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.build_set_speed_actions(speed)
    });
    execute_actions(
        actions,
        &*ctx.rodio,
        &ctx.db,
        &ctx.player_state,
        &ctx.sinks,
    );
    Ok(())
}

pub fn player_set_gapless(ctx: &PlaybackContext, enabled: bool) -> Result<(), AppError> {
    with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.gapless_enabled = enabled;
    });
    Ok(())
}

// --- Graphic equalizer -----------------------------------------------------
//
// EQ state lives on the Rodio backend's lock-free shared cell, not the
// `PlayerState` machine, so these bypass `with_state_emit` / `execute_actions`
// and write the shared cell directly — the same place `set_volume`/`set_speed`
// ultimately land. They're synchronous and infallible (no decode, no I/O), and
// apply to both the playing track and the gapless-preloaded one at once.

/// Toggle the graphic equalizer on the live player.
pub fn player_set_eq_enabled(ctx: &PlaybackContext, enabled: bool) {
    ctx.rodio.set_eq_enabled(enabled);
}

/// Set a single EQ band's gain (dB) on the live player.
pub fn player_set_eq_band(ctx: &PlaybackContext, index: usize, gain_db: f32) {
    ctx.rodio.set_eq_band(index, gain_db);
}

/// Replace all EQ band gains on the live player (preset / reset / hydration).
pub fn player_set_eq_gains(ctx: &PlaybackContext, gains: &[f32]) {
    ctx.rodio.set_eq_gains(gains);
}

/// Set the EQ preamp / master gain (dB) on the live player.
pub fn player_set_eq_preamp(ctx: &PlaybackContext, preamp_db: f32) {
    ctx.rodio.set_eq_preamp(preamp_db);
}

#[cfg(test)]
#[path = "tests/playback_tests.rs"]
mod tests;
