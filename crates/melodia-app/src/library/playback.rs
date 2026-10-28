use std::sync::Arc;
use std::time::Duration;

use crate::state::PlaybackContext;
use melodia_audio::player::source::stream_source;
use melodia_core::error::AppError;
use melodia_core::error::describe;
use melodia_engine::player::engine::backend::{OutputChoice, PlaybackEngine};
use melodia_engine::player::engine::queue::window_slots;
use melodia_engine::player::engine::state::{
    MAX_VOLUME, PlayerAction, PlayerState, lock_state, with_state_emit,
};
use melodia_engine::player::engine::types::{PlaybackSource, PlaybackStatus, RadioNowPlaying};
use melodia_playback::player::playback::output::convert::Resampler;
use melodia_playback::player::playback::output::{self, OutputDevice};
use melodia_store::database::queries;

/// Which slot of `queued` playback should start on, resolved through the id: the queue drops ids
/// that no longer exist, so an index into `track_ids` shifts past the first gap. `None` when no row
/// was picked, the index is past the end, or the picked row didn't survive.
fn resolve_start_slot(
    track_ids: &[i64],
    queued: &[i64],
    start_index: Option<usize>,
) -> Option<usize> {
    let wanted = start_index.and_then(|i| track_ids.get(i).copied())?;
    queued.iter().position(|&id| id == wanted)
}

/// Replace the queue with `track_ids` and start at `start_index` (`None` = head). Every browsing
/// surface starts playback here, so the queue is always the list the user was looking at.
///
/// With shuffle on, the rest is shuffled behind the chosen track: a freshly seeded `play_order` is
/// the identity, so the lit shuffle button would otherwise walk the album straight through.
/// `original_order` stays as seeded, so turning shuffle off restores display order.
pub async fn player_play_tracks(
    ctx: &PlaybackContext,
    track_ids: Vec<i64>,
    start_index: Option<usize>,
) -> Result<(), AppError> {
    let queued = crate::library::keyset::existing_in_order(&ctx.db, &track_ids).await?;
    if queued.is_empty() {
        return Err(AppError::queue_msg("No valid tracks provided"));
    }

    let start = resolve_start_slot(&track_ids, &queued, start_index).unwrap_or_else(|| {
        match start_index {
            None => {}
            Some(i) if i >= track_ids.len() => log::warn!(
                "play_tracks: start_index {i} is past the {} ids handed in; starting at the head",
                track_ids.len()
            ),
            Some(_) => log::warn!(
                "play_tracks: the picked track didn't survive the fetch; starting at the head"
            ),
        }
        0
    });
    // Fetched here so neither the start nor an immediate skip waits on the window task; under
    // shuffle the task fetches the new neighbours.
    let wraps = lock_state(&ctx.player_state).queue.repeat_mode.wraps();
    let near: Vec<i64> = window_slots(queued.len(), Some(start), wraps)
        .filter_map(|slot| queued.get(slot).copied())
        .collect();
    let records =
        queries::track::get_track_summaries_by_ids(&ctx.db, &near).await?.into_iter().map(Arc::new);
    ctx.emit_and_execute(|s| {
        s.queue.clear();
        s.queue.add_tracks(&queued);
        s.queue.current_index = Some(start);
        if s.queue.shuffle_enabled {
            s.queue
                .shuffle_play_order_in_place(&mut rand::rng(), /* anchor_to_current */ true);
        }
        s.window.seat(&s.queue.window_ids(), records);

        match s.queue.current_id() {
            Some(id) => s.start_queued(id, false, 0),
            None => vec![],
        }
    });
    Ok(())
}

pub fn player_play(ctx: &PlaybackContext) -> Result<(), AppError> {
    if resume_station(ctx) {
        return Ok(());
    }
    ctx.emit_and_execute(|s| play_actions(s, &ctx.engine));
    Ok(())
}

/// Resume the paused track, or start it over where a pause long enough to give an exclusive device
/// back left its deck empty. The deck is asked under the state lock, the lock order's own
/// direction, so no other emit can split the answer from the actions it picks.
fn play_actions(s: &mut PlayerState, engine: &PlaybackEngine) -> Vec<PlayerAction> {
    if s.status == PlaybackStatus::Paused && !engine.holds_source() {
        return s.build_replay_actions();
    }
    s.build_play_actions()
}

/// Whether a play command has to re-open a station instead of resuming the deck: `Paused` is the
/// only state holding a station without a connection. Pure, so the routing is testable without a
/// socket.
pub(crate) fn needs_station_reopen(status: PlaybackStatus, has_station: bool) -> bool {
    has_station && status == PlaybackStatus::Paused
}

/// Re-open the station the player is paused on, if any; `true` means it took over.
///
/// A paused station has no connection, so resuming is a fresh open: a network round trip, spawned
/// off the state lock, every transport caller already being inside `runtime.spawn`. The predicate
/// and the connecting transition share **one** emit, or a `Stop` in the gap would be undone by the
/// connect already decided on.
fn resume_station(ctx: &PlaybackContext) -> bool {
    let mut resuming = None;
    ctx.emit_and_execute(|s| {
        if !needs_station_reopen(s.status, s.station().is_some()) {
            return vec![];
        }
        let Some(station) = s.station().cloned() else {
            return vec![];
        };
        let (generation, actions) = s.build_station_connecting_actions(station.clone());
        resuming = Some((generation, station));
        actions
    });

    let Some((generation, station)) = resuming else {
        return false;
    };

    let ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = open_and_start_station(&ctx, &station, generation).await {
            log::warn!("Could not resume {}: {}", station.name, describe(&e));
        }
    });
    true
}

/// Tune to `station`, leaving the queue untouched underneath.
///
/// Two transitions around the open, and the second is checked against the first's generation, so a
/// station the user has already moved off doesn't start playing seconds later.
pub async fn player_play_station(
    ctx: &PlaybackContext,
    station: &Arc<RadioNowPlaying>,
) -> Result<(), AppError> {
    let mut generation = 0;
    ctx.emit_and_execute(|s| {
        let (session, actions) = s.build_station_connecting_actions(station.clone());
        generation = session;
        actions
    });

    open_and_start_station(ctx, station, generation).await
}

/// The network half of a tune, split out so [`resume_station`] can take the connecting transition
/// under its predicate's emit.
async fn open_and_start_station(
    ctx: &PlaybackContext,
    station: &RadioNowPlaying,
    generation: u64,
) -> Result<(), AppError> {
    let client = ctx.http.get_or_init(melodia_net::services::net::build_http_client).clone();
    let opened = stream_source::open(&client, &station.stream_url).await;

    match opened {
        Ok(prepared) => {
            let codec = prepared.codec().to_owned();
            ctx.engine.stage_stream(generation, prepared);
            ctx.emit_and_execute(|s| {
                s.adopt_probed_codec(generation, &codec);
                s.build_station_connected_actions(generation)
            });
            // A session that ended mid-open declined the emit above and left the stage unclaimed;
            // an abandoned connection must not outlive its station.
            ctx.engine.discard_staged_stream(generation);
            crate::library::radio::record_probed_codec(&ctx.db, station, &codec).await;
            Ok(())
        }
        Err(e) => {
            ctx.emit_and_execute(|s| s.build_station_failed_actions(generation));
            melodia_core::utils::toast::notify(
                melodia_core::utils::toast::ToastKind::PlaybackFailed,
                station.name.clone(),
            );
            Err(e)
        }
    }
}

/// Fade length for a transport pause or stop, `0` with the setting off. Everything reaching the
/// four transport commands fades, the sleep timer's expiry included.
///
/// What the machine does for its own reasons passes `0` directly instead: `stop_end_of_queue`, and
/// the `Pause` next/previous append to restore a paused deck, where a fade would make the incoming
/// track audible.
fn transport_fade_ms(ctx: &PlaybackContext) -> u64 {
    if ctx.engine.crossfade_settings().fade_on_pause {
        melodia_playback::player::playback::crossfade::PAUSE_FADE_MS
    } else {
        0
    }
}

pub fn player_pause(ctx: &PlaybackContext) -> Result<(), AppError> {
    let fade_ms = transport_fade_ms(ctx);
    ctx.emit_and_execute(move |s| s.build_pause_actions(fade_ms));
    Ok(())
}

/// Branches inside the state lock, so two near-simultaneous toggles (UI and media key) can't race
/// past each other.
pub fn player_toggle_play_pause(ctx: &PlaybackContext) -> Result<(), AppError> {
    if resume_station(ctx) {
        return Ok(());
    }
    let fade_ms = transport_fade_ms(ctx);
    let engine = &ctx.engine;
    ctx.emit_and_execute(move |s| match s.status {
        PlaybackStatus::Playing | PlaybackStatus::Loading => s.build_pause_actions(fade_ms),
        PlaybackStatus::Paused | PlaybackStatus::Stopped => play_actions(s, engine),
    });
    Ok(())
}

/// User-initiated stop, from the play button's hold and the OS media controls alike: back to the
/// top with the track still seated, so `player_play` starts it over, as MPRIS specifies.
pub fn player_stop(ctx: &PlaybackContext) -> Result<(), AppError> {
    let fade_ms = transport_fade_ms(ctx);
    ctx.emit_and_execute(move |s| s.build_stop_actions(fade_ms));
    Ok(())
}

/// Stop a live stream and nothing else, for the Radio switch going off. Checked under the state
/// lock, since a read-then-stop would stop a track started in between. Ending the session keeps a
/// connect in flight from landing afterwards.
pub fn player_stop_station(ctx: &PlaybackContext) -> Result<(), AppError> {
    let fade_ms = transport_fade_ms(ctx);
    ctx.emit_and_execute(move |s| {
        if s.station().is_some() { s.build_stop_actions(fade_ms) } else { Vec::new() }
    });
    Ok(())
}

pub fn player_seek(ctx: &PlaybackContext, position_ms: u64) -> Result<(), AppError> {
    ctx.emit_and_execute(|s| s.build_seek_actions(position_ms));
    Ok(())
}

pub fn player_next(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(melodia_engine::player::engine::state::PlayerState::build_next_actions);
    Ok(())
}

pub fn player_previous(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(
        melodia_engine::player::engine::state::PlayerState::build_previous_actions,
    );
    Ok(())
}

/// Live only: a drag fires this continuously, and [`commit_player_settings`] persists on release.
pub fn player_set_volume(ctx: &PlaybackContext, level: u32) -> Result<(), AppError> {
    // Duplicate slider emits stop here, short of a view model.
    {
        let s = lock_state(&ctx.player_state);
        if s.holds_volume(level) {
            return Ok(());
        }
    }
    ctx.emit_and_execute(|s| s.build_set_volume_actions(level));
    Ok(())
}

/// Steps the volume by `delta` from the engine's own level. The volume keys and the wheel come
/// through here, persisting through `commit_player_settings` once a burst settles.
pub fn player_step_volume(ctx: &PlaybackContext, delta: i32) -> Result<(), AppError> {
    // A wheel spun against either end would otherwise emit a view model per notch.
    {
        let s = lock_state(&ctx.player_state);
        if s.holds_volume(s.stepped_volume(delta)) {
            return Ok(());
        }
    }
    ctx.emit_and_execute(|s| s.build_step_volume_actions(delta));
    Ok(())
}

/// [`player_set_volume`] and its commit at once, for a move that arrives whole rather than as a
/// drag: the OS media panel's, or the device's own control's.
pub async fn player_set_volume_committed(
    ctx: &PlaybackContext,
    level: u32,
) -> Result<(), AppError> {
    player_set_volume(ctx, level)?;
    commit_player_settings(ctx).await
}

pub async fn player_toggle_mute(ctx: &PlaybackContext) -> Result<(), AppError> {
    ctx.emit_and_execute(
        melodia_engine::player::engine::state::PlayerState::build_toggle_mute_actions,
    );
    commit_player_settings(ctx).await
}

/// Persist the current volume and mute once per gesture: the slider's release, a settled run of key
/// or wheel steps, a mute, or [`player_set_volume_committed`]. Unchanged values write nothing.
pub async fn commit_player_settings(ctx: &PlaybackContext) -> Result<(), AppError> {
    let (volume, is_muted) = {
        let s = lock_state(&ctx.player_state);
        (s.volume, s.is_muted)
    };
    let paths = ctx.paths.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        crate::services::settings::mutate_settings_if(&paths, |s| {
            let changed = s.volume != volume || s.playback.is_muted != is_muted;
            s.volume = volume;
            s.playback.is_muted = is_muted;
            changed
        })
    })
    .await
    .map_err(|e| AppError::io("commit_player_settings join", e))?
}

pub fn player_set_playback_speed(ctx: &PlaybackContext, speed: f64) -> Result<(), AppError> {
    ctx.emit_and_execute(|s| s.build_set_speed_actions(speed));
    Ok(())
}

/// [`player_set_playback_speed`] and the write that has it survive a restart, as
/// [`player_set_volume_committed`] is for the volume.
pub async fn player_set_playback_speed_committed(
    ctx: &PlaybackContext,
    speed: f64,
) -> Result<(), AppError> {
    player_set_playback_speed(ctx, speed)?;
    let paths = Arc::clone(&ctx.paths);
    tokio::task::spawn_blocking(move || crate::library::settings::set_playback_speed(&paths, speed))
        .await
        .map_err(AppError::io_source)?
}

pub fn player_set_gapless(ctx: &PlaybackContext, enabled: bool) -> Result<(), AppError> {
    with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.gapless_enabled = enabled;
    });
    Ok(())
}

/// Arm the sleep timer's "end of current track": the playback monitor pauses at the next end of
/// stream instead of advancing, and disarms it as it fires. Session-only.
///
/// **Refused while a station plays**: a live source has no track end, so the row would read "Track
/// end" over a timer that never fires. Guarded here rather than at the caller so no later caller
/// can arm it.
pub fn player_set_pause_at_track_end(ctx: &PlaybackContext, armed: bool) -> Result<(), AppError> {
    with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
        s.pause_after_current_track = armed && s.source_allows(PlaybackSource::has_known_duration);
    });
    Ok(())
}

// The EQ, ReplayGain and crossfade setters write the engine's lock-free cells rather than
// `PlayerState`, so they skip the emit and can't fail.

pub fn player_set_eq_enabled(ctx: &PlaybackContext, enabled: bool) {
    ctx.engine.set_eq_enabled(enabled);
}

pub fn player_set_eq_band(ctx: &PlaybackContext, index: usize, gain_db: f32) {
    ctx.engine.set_eq_band(index, gain_db);
}

pub fn player_set_eq_gains(ctx: &PlaybackContext, gains: &[f32]) {
    ctx.engine.set_eq_gains(gains);
}

pub fn player_set_eq_preamp(ctx: &PlaybackContext, preamp_db: f32) {
    ctx.engine.set_eq_preamp(preamp_db);
}

// The per-track gain is baked per source at play time (`player::playback::replaygain`).

pub fn player_set_replaygain_enabled(ctx: &PlaybackContext, enabled: bool) {
    ctx.engine.set_replaygain_enabled(enabled);
}

pub fn player_set_replaygain_mode(
    ctx: &PlaybackContext,
    mode: melodia_playback::player::playback::replaygain::RgMode,
) {
    ctx.engine.set_replaygain_mode(mode);
}

pub fn player_set_replaygain_preamp(ctx: &PlaybackContext, preamp_db: f32) {
    ctx.engine.set_replaygain_preamp(preamp_db);
}

/// The static, peak-based clip guard.
pub fn player_set_replaygain_prevent_clipping(ctx: &PlaybackContext, on: bool) {
    ctx.engine.set_replaygain_prevent_clipping(on);
}

pub fn player_set_crossfade_enabled(ctx: &PlaybackContext, enabled: bool) {
    ctx.engine.set_crossfade_enabled(enabled);
}

/// Clamped by the backend.
pub fn player_set_crossfade_duration_ms(ctx: &PlaybackContext, ms: u32) {
    ctx.engine.set_crossfade_duration_ms(ms);
}

/// Also crossfade on a manual track change (next / previous / picking a track).
pub fn player_set_crossfade_manual(ctx: &PlaybackContext, on: bool) {
    ctx.engine.set_crossfade_manual(on);
}

/// Leave same-album transitions gapless.
pub fn player_set_crossfade_skip_same_album(ctx: &PlaybackContext, on: bool) {
    ctx.engine.set_crossfade_skip_same_album(on);
}

/// Fade out on pause / user stop, and fade back in on resume.
pub fn player_set_crossfade_fade_on_pause(ctx: &PlaybackContext, on: bool) {
    ctx.engine.set_crossfade_fade_on_pause(on);
}

/// Whether following the file's rate can change anything here, so the toggle is offered at all.
pub const FOLLOW_RATE_SUPPORTED: bool = output::FOLLOW_RATE_SUPPORTED;

/// Open the output at each track's own sample rate, from the next track on.
pub fn player_set_follow_rate(ctx: &PlaybackContext, on: bool) {
    ctx.engine.set_follow_rate(on);
}

/// Give an exclusive device back once a pause has held it long enough, from the next pause on.
pub fn player_set_release_when_paused(ctx: &PlaybackContext, on: bool) {
    ctx.engine.set_release_when_paused(on);
}

/// Write `hold` of silence after each reopen onto a new rate, from the next one on. Blocking: it
/// waits out any reopen in flight, so it belongs on the blocking pool.
pub fn player_set_resync_hold(ctx: &PlaybackContext, hold: Duration) {
    ctx.engine.set_resync_hold(hold);
}

/// Convert with `resampler` from the next block on, mid-track included. Blocking: it takes the
/// decks lock a reopen holds, so it belongs on the blocking pool.
pub fn player_set_resampler(ctx: &PlaybackContext, resampler: Resampler) {
    ctx.engine.set_resampler(resampler);
}

/// Whether this platform has an exclusive backend. Where it doesn't, an exclusive choice falls
/// back to shared on every open, so the picker isn't offered at all.
pub const EXCLUSIVE_SUPPORTED: bool = output::EXCLUSIVE_SUPPORTED;

/// Whether an exclusive claim can be polled instead of event-driven here, which only matters to
/// the drivers that stutter under events.
pub const POLLING_SUPPORTED: bool = output::POLLING_SUPPORTED;

/// Whether an exclusive claim can carry the volume on the device's own control here.
pub const HARDWARE_VOLUME_SUPPORTED: bool = output::HARDWARE_VOLUME_SUPPORTED;

/// Whether an exclusive claim can keep a device lacking the file's rate by converting to one it
/// has, here.
pub const RATE_FALLBACK_SUPPORTED: bool = output::RATE_FALLBACK_SUPPORTED;

/// Whether the device picker chooses where shared output plays too, rather than exclusive alone.
pub const SHARED_DEVICE_SUPPORTED: bool = output::SHARED_DEVICE_SUPPORTED;

/// The devices an exclusive claim can be aimed at, and where [`SHARED_DEVICE_SUPPORTED`] a shared
/// stream too. Blocking: it asks every device.
pub fn output_devices() -> Vec<OutputDevice> {
    output::devices()
}

/// Take the card for ourselves or give it back, now. Blocking: it opens a device, so it belongs
/// on the blocking pool rather than the UI thread.
pub fn player_set_output_choice(ctx: &PlaybackContext, choice: OutputChoice) {
    ctx.engine.set_output_choice(choice);
}

/// Turn off everything the user chose that changes the samples, and follow the file's rate,
/// returning the volume it left: the live half of `library::settings::reset_for_bit_perfect`.
///
/// **A volume on the device's own control is left where it is**, only unmuted: it changes no
/// sample, and raising it would raise the user's speakers. Blocking: a reopen holds the output.
pub fn player_make_bit_perfect(ctx: &PlaybackContext) -> u32 {
    player_set_eq_enabled(ctx, false);
    player_set_replaygain_enabled(ctx, false);
    let volume_on_device =
        ctx.engine.negotiated().is_some_and(|negotiated| negotiated.hardware_volume);
    let mut volume = MAX_VOLUME;
    ctx.emit_and_execute(|s| {
        if volume_on_device {
            volume = s.volume;
        }
        let mut actions = s.build_set_speed_actions(1.0);
        actions.extend(s.build_set_volume_actions(volume));
        actions
    });
    player_set_follow_rate(ctx, true);
    volume
}

// No visualizer setter: the Now Playing view arms the tap, the setting only mounting the strip.

#[cfg(test)]
#[path = "tests/playback_tests.rs"]
mod tests;
