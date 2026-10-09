use std::collections::VecDeque;
use std::path::Path;

use melodia_core::error::{AppError, describe};
use melodia_core::utils::play_counts::{PlayCountEvent, try_send};

use super::backend::PlayerBackend;
use super::event_sink::PlayerSinks;
use super::state::{
    PlayerAction, PlayerState, PlayerStateHandle, play_track_inner, stop_end_of_queue,
    with_state_emit,
};
use melodia_playback::player::playback::replaygain::TrackReplayGain;

/// Execute a list of `PlayerActions` against the audio backend and database.
/// Called after releasing the `PlayerState` lock.
///
/// `PlayMedia` is pre-flighted with `Path::exists()` and any decode failure
/// also auto-skips: the queue's `build_next_actions` is computed inline and
/// appended to the pending action set so a single stale double-click within
/// the watcher debounce window doesn't dead-end playback. The bad track
/// stays in the queue until the watcher catches up and `tasks::queue_prune`
/// removes it, which for a file outside the watched folders is never.
pub fn execute_actions<B: PlayerBackend>(
    actions: Vec<PlayerAction>,
    engine: &B,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
) {
    let mut pending = Pending { actions: actions.into(), skipped: 0 };
    while let Some(action) = pending.actions.pop_front() {
        // Safe per action because nothing periodic reaches here: the position
        // tick decides in `evaluate_playing_tick` and the 30 s queue save writes
        // its file directly.
        log::debug!("player: {action}");
        match action {
            PlayerAction::PlayMedia { file_path, volume, speed, start_position_ms, replaygain } => {
                start_or_skip(
                    &mut pending,
                    engine,
                    player_state,
                    sinks,
                    &file_path,
                    StartMode::Fresh,
                    || engine.play_media(&file_path, volume, speed, start_position_ms, replaygain),
                );
            }
            PlayerAction::BeginCrossfade { file_path, replaygain, fade_ms, volume, speed } => {
                // `build_crossfade_actions` already advanced onto this track, so
                // the `advance_skip` a failure triggers correctly lands on the one
                // after it. In `RepeatMode::One` that also steps off the repeated
                // track — rare enough (the file vanished mid-play) to accept.
                start_or_skip(
                    &mut pending,
                    engine,
                    player_state,
                    sinks,
                    &file_path,
                    StartMode::Crossfade,
                    || engine.begin_crossfade(&file_path, replaygain, fade_ms, volume, speed),
                );
            }
            PlayerAction::Resume => engine.resume(),
            PlayerAction::Pause { fade_ms } => engine.pause_with_fade(fade_ms),
            PlayerAction::Stop { fade_ms } => engine.stop_with_fade(fade_ms),
            PlayerAction::Seek { position_ms, file_path, replaygain } => {
                engine.seek(&file_path, position_ms, replaygain);
                if let Some(mc) = sinks.media_controls.as_ref() {
                    mc.seeked(position_ms);
                }
            }
            PlayerAction::SetVolume(v) => engine.set_volume(v),
            PlayerAction::SetSpeed(s) => engine.set_speed(s),
            PlayerAction::PreloadGapless(path) => {
                // This action only ever *clears* a stale preload (`path` is
                // `None`); the real gapless preload with baked ReplayGain happens
                // directly in `spawn_playback_monitor`. A default (unity) RG here
                // is harmless — the `None` path never builds an audio source.
                engine.preload_gapless(path.as_deref(), TrackReplayGain::default());
            }
            PlayerAction::PlayStream { generation, volume } => {
                // Deliberately not through `start_or_skip`: there is no file to pre-flight with
                // `Path::exists`, and nothing to auto-skip onto — a station is one source, not a
                // position in a queue.
                if let Err(e) = engine.play_stream(generation, volume) {
                    log::error!("Failed to start the radio stream: {}", describe(&e));
                    engine.stop();
                    enqueue_station_failure(&mut pending, player_state, sinks, generation);
                }
            }
            // Both no-op until `boot::tasks` installs the flusher, which it does before playback
            // can start. Nothing here writes the row itself: a `DbPool` on this side is what pins
            // the engine to sqlx.
            PlayerAction::UpdatePlayCount(track_id) => {
                try_send(PlayCountEvent::Play(track_id));
            }
            PlayerAction::UpdateSkipCount(track_id) => {
                try_send(PlayCountEvent::Skip(track_id));
            }
        }
    }
}

/// Mutate state, publish the `ViewModel`, then execute the resulting side
/// effects — all while holding the per-`PlayerStateHandle` execution lock so
/// mutation order equals side-effect order across tokio workers.
///
/// This is the serialized replacement for the bare `with_state_emit(...)` +
/// `execute_actions(...)` pair. `with_state_emit` alone keeps each *mutation*
/// atomic, but the `execute_actions` that follows runs on whatever worker the
/// caller is on; two batches from different tasks (e.g. the playback monitor's
/// EOS-advance and a UI `Stop`) could otherwise interleave their side effects
/// and leave state and backend disagreeing (a rare TOCTOU). Holding `exec_lock`
/// across both halves closes that window.
///
/// The lock spans only synchronous work (no `.await`), so a blocking mutex is
/// correct. `enqueue_auto_skip`'s nested `with_state_emit` inside
/// `execute_actions` takes the *state* mutex, never `exec_lock`, so there is no
/// re-entrancy.
pub fn emit_and_execute<B, F>(
    engine: &B,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    f: F,
) where
    B: PlayerBackend,
    F: FnOnce(&mut PlayerState) -> Vec<PlayerAction>,
{
    let _exec = player_state.lock_exec();
    let actions = with_state_emit(player_state, sinks, f);
    execute_actions(actions, engine, player_state, sinks);
}

/// What is left to run of one [`execute_actions`] call.
struct Pending {
    actions: VecDeque<PlayerAction>,
    /// Tracks this call has skipped as unplayable. A repeating queue wraps on a skip, so without
    /// a bound a queue with nothing playable in it skips forever, holding the lock every
    /// transport control waits on.
    skipped: usize,
}

impl Pending {
    /// Run `actions` next, in their order, ahead of anything still queued from the batch.
    fn prepend(&mut self, actions: Vec<PlayerAction>) {
        for (i, a) in actions.into_iter().enumerate() {
            self.actions.insert(i, a);
        }
    }
}

/// How a track is being started — the only thing that differs between the
/// [`PlayerAction::PlayMedia`] and [`PlayerAction::BeginCrossfade`] arms of
/// [`execute_actions`].
#[derive(Copy, Clone)]
enum StartMode {
    /// Takes over the decks outright. A failure leaves them half-set, so stop
    /// before skipping on.
    Fresh,
    /// Overlaps the track still playing on the other deck. Deliberately does
    /// **not** stop on failure: the outgoing track is still audible, and the
    /// `play_media` that the auto-skip produces takes over from it cleanly either
    /// way — hard-cutting (which clears both decks) or, with `crossfade_manual`
    /// on, fading out of it. Stopping here would only insert a gap of silence
    /// ahead of that.
    Crossfade,
}

impl StartMode {
    fn stops_on_failure(self) -> bool {
        matches!(self, Self::Fresh)
    }

    /// The verb for the decode-failure log: "Failed to *play* …" / "Failed to
    /// *crossfade into* …".
    fn verb(self) -> &'static str {
        match self {
            Self::Fresh => "play",
            Self::Crossfade => "crossfade into",
        }
    }

    /// The transition the skip happened at, for the vanished-file log.
    fn at(self) -> &'static str {
        match self {
            Self::Fresh => "playback",
            Self::Crossfade => "crossfade",
        }
    }
}

/// Start a track on the backend, auto-skipping past it if it can't be played.
///
/// Shared by the two start actions. A file that has vanished is skipped
/// *silently* while the skip lands on something playable — the usual cause is a
/// stale double-click inside the watcher's debounce window — and toasts only
/// when nothing in the queue plays. A decode failure is louder: the music
/// silently stopping is otherwise invisible, so it always toasts.
fn start_or_skip<B: PlayerBackend>(
    pending: &mut Pending,
    engine: &B,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    file_path: &str,
    mode: StartMode,
    start: impl FnOnce() -> Result<(), AppError>,
) {
    if !Path::new(file_path).exists() {
        log::warn!("Skipping vanished file at {}: {file_path}", mode.at());
        if mode.stops_on_failure() {
            engine.stop();
        }
        if enqueue_auto_skip(pending, player_state, sinks) {
            toast_playback_failed(file_path);
        }
        return;
    }
    if let Err(e) = start() {
        log::error!("Failed to {} {file_path}: {}", mode.verb(), describe(&e));
        toast_playback_failed(file_path);
        if mode.stops_on_failure() {
            engine.stop();
        }
        enqueue_auto_skip(pending, player_state, sinks);
    }
}

fn toast_playback_failed(file_path: &str) {
    melodia_core::utils::toast::notify(
        melodia_core::utils::toast::ToastKind::PlaybackFailed,
        toast_track_name(file_path),
    );
}

/// The file name alone, for the failure toast — the full path is too long to
/// read in a toast. Falls back to the whole path when there is no file name.
fn toast_track_name(file_path: &str) -> String {
    Path::new(file_path)
        .file_name()
        .map_or_else(|| file_path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Clear a station whose staged stream could not be started, so the transport doesn't sit on a
/// `Loading` that will never resolve.
///
/// No toast: the only ways this fires are a stage that was never made and one made for a station
/// the user has already moved off, neither of which is theirs to act on. A stream that failed to
/// *open* is the user-visible case, and `library::radio` says so there.
fn enqueue_station_failure(
    pending: &mut Pending,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    generation: u64,
) {
    pending.prepend(with_state_emit(player_state, sinks, |s| {
        s.build_station_failed_actions(generation)
    }));
}

/// Advance past a track that turned out to be unplayable (missing or
/// undecodable) and run whatever that produces next. Returns whether it gave up
/// instead.
///
/// It gives up once the batch has skipped a whole lap of the queue, since
/// nothing left in it will play. On an empty queue this emits `Stop` only.
/// Either way the `with_state_emit` call took the lock to do it, so the state
/// machine and `ViewModel` both reflect the end-of-queue state.
fn enqueue_auto_skip(
    pending: &mut Pending,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
) -> bool {
    pending.skipped += 1;
    let skipped = pending.skipped;
    let mut gave_up = false;
    let actions = with_state_emit(player_state, sinks, |s| {
        if !s.queue.is_empty() && skipped >= s.queue.len() {
            gave_up = true;
            return stop_end_of_queue(s);
        }
        match s.queue.advance_skip().cloned() {
            Some(track) => play_track_inner(s, track, None),
            None => stop_end_of_queue(s),
        }
    });
    if gave_up {
        log::warn!("Stopping: none of the {skipped} track(s) tried could be played");
    }
    pending.prepend(actions);
    gave_up
}

#[cfg(test)]
#[path = "tests/actions_tests.rs"]
mod tests;
