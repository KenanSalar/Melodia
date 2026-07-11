use std::sync::Arc;

use crate::database::queries;
use crate::entities::track::TrackSummary;
use crate::error::AppError;
use crate::player::state::{lock_state, play_track_inner, with_state_emit};
use crate::player::types::PlaybackStatus;
use crate::state::PlaybackContext;

pub async fn player_play_track(ctx: &PlaybackContext, track_id: i64) -> Result<(), AppError> {
    let summary = queries::track::get_track_summary_by_id(&ctx.db, track_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("track {track_id}")))?;
    let summary = Arc::new(summary);

    ctx.emit_and_execute(|s| {
        s.queue.set_direct_play(Arc::clone(&summary));
        play_track_inner(s, summary, None)
    });
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
    ctx.emit_and_execute(|s| {
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
    Ok(())
}

pub fn player_play(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(crate::player::state::PlayerState::build_play_actions);
    Ok(())
}

pub fn player_pause(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(crate::player::state::PlayerState::build_pause_actions);
    Ok(())
}

/// User-initiated stop — preserves `current_track` and `position_ms` so `player_play` can resume
/// from where the user stopped. Contrast with `stop_end_of_queue` which resets position to 0.
/// Toggle play/pause based on current playback status. Branching happens inside
/// the state lock so two near-simultaneous toggles (e.g. UI + media-key) can't
/// race past each other.
pub fn player_toggle_play_pause(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(|s| match s.status {
        PlaybackStatus::Playing | PlaybackStatus::Loading => s.build_pause_actions(),
        PlaybackStatus::Paused | PlaybackStatus::Stopped => s.build_play_actions(),
    });
    Ok(())
}

pub fn player_stop(ctx: &PlaybackContext) -> Result<(), AppError> {
    // A user-initiated stop fades out when the setting is on. Internal stops
    // (end of queue, error recovery) go through `stop_end_of_queue`, which
    // always passes `0` — there is no audio left to fade there.
    let fade_ms = if ctx.rodio.crossfade_settings().fade_on_pause {
        crate::player::crossfade::PAUSE_FADE_MS
    } else {
        0
    };
    ctx.emit_and_execute(move |s| s.build_stop_actions(fade_ms));
    Ok(())
}

pub fn player_seek(ctx: &PlaybackContext, position_ms: u64) -> Result<(), AppError> {
    ctx.emit_and_execute(|s| s.build_seek_actions(position_ms));
    Ok(())
}

pub fn player_next(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(crate::player::state::PlayerState::build_next_actions);
    Ok(())
}

pub fn player_previous(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(crate::player::state::PlayerState::build_previous_actions);
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
    ctx.emit_and_execute(|s| s.build_set_volume_actions(level));
    // Persistence is intentionally NOT triggered here. The slider fires
    // `set_volume` continuously during drag for live audio; settings.json
    // is flushed once via `commit_player_settings` on slider release.
    Ok(())
}

pub async fn player_set_muted(ctx: &PlaybackContext, muted: bool) -> Result<(), AppError> {
    ctx.emit_and_execute(|s| s.build_set_muted_actions(muted));
    // Mute is single-tap; commit inline.
    commit_player_settings(ctx).await
}

pub async fn player_toggle_mute(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(crate::player::state::PlayerState::build_toggle_mute_actions);
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
    ctx.emit_and_execute(|s| s.build_set_speed_actions(speed));
    Ok(())
}

pub fn player_set_gapless(ctx: &PlaybackContext, enabled: bool) -> Result<(), AppError> {
    with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.gapless_enabled = enabled;
    });
    Ok(())
}

/// Arm / disarm the sleep-timer's "End of current track" mode. When armed, the
/// playback monitor pauses at the next end-of-stream boundary instead of
/// advancing the queue (see `src/player/handlers.rs`). Session-only — nothing
/// is persisted. The `with_state_emit` re-publishes the light `ViewModel` so the
/// UI's `Player.vm.sleep_at_track_end` (and thus the overflow-menu sleep row)
/// tracks the flag; the monitor disarms it when it fires, which re-emits and
/// auto-clears the row.
pub fn player_set_pause_at_track_end(ctx: &PlaybackContext, armed: bool) -> Result<(), AppError> {
    with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.pause_after_current_track = armed;
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

// --- ReplayGain ------------------------------------------------------------
//
// ReplayGain master state (enabled / mode / preamp / prevent-clipping) lives on
// the same lock-free shared cell as the EQ, so these setters mirror the EQ ones:
// synchronous, infallible, and applied to the playing + gapless-preloaded track
// at once. The *per-track* gain is baked per source at play time (see
// `player::replaygain`), not set here.

/// Toggle `ReplayGain` on the live player.
pub fn player_set_replaygain_enabled(ctx: &PlaybackContext, enabled: bool) {
    ctx.rodio.set_replaygain_enabled(enabled);
}

/// Set the `ReplayGain` mode (Track / Album) on the live player.
pub fn player_set_replaygain_mode(ctx: &PlaybackContext, mode: crate::player::replaygain::RgMode) {
    ctx.rodio.set_replaygain_mode(mode);
}

/// Set the `ReplayGain` preamp (dB) on the live player.
pub fn player_set_replaygain_preamp(ctx: &PlaybackContext, preamp_db: f32) {
    ctx.rodio.set_replaygain_preamp(preamp_db);
}

/// Toggle the static peak-based clip guard on the live player.
pub fn player_set_replaygain_prevent_clipping(ctx: &PlaybackContext, on: bool) {
    ctx.rodio.set_replaygain_prevent_clipping(on);
}

// --- Crossfade -------------------------------------------------------------
//
// Crossfade settings live on a lock-free shared cell like the EQ and ReplayGain
// ones, so these setters are synchronous and infallible. Unlike those two the
// cell is read by the *control* layer (this backend and the playback monitor),
// never by the audio thread — the per-deck ramp it drives lives in `FadeShared`.

/// Toggle crossfade on the live player.
pub fn player_set_crossfade_enabled(ctx: &PlaybackContext, enabled: bool) {
    ctx.rodio.set_crossfade_enabled(enabled);
}

/// Set the crossfade length (ms) on the live player. Clamped by the backend.
pub fn player_set_crossfade_duration_ms(ctx: &PlaybackContext, ms: u32) {
    ctx.rodio.set_crossfade_duration_ms(ms);
}

/// Also crossfade on a manual track change (next / previous / picking a track).
pub fn player_set_crossfade_manual(ctx: &PlaybackContext, on: bool) {
    ctx.rodio.set_crossfade_manual(on);
}

/// Leave same-album transitions gapless.
pub fn player_set_crossfade_skip_same_album(ctx: &PlaybackContext, on: bool) {
    ctx.rodio.set_crossfade_skip_same_album(on);
}

/// Fade out on pause / user stop, and fade back in on resume.
pub fn player_set_crossfade_fade_on_pause(ctx: &PlaybackContext, on: bool) {
    ctx.rodio.set_crossfade_fade_on_pause(on);
}

#[cfg(test)]
#[path = "tests/playback_tests.rs"]
mod tests;
