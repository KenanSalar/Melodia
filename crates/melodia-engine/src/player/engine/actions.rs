use std::collections::VecDeque;
use std::path::Path;

use melodia_core::error::{AppError, describe};
use melodia_core::utils::play_counts::{PlayCountEvent, try_send};

use super::backend::PlayerBackend;
use super::event_sink::PlayerSinks;
use super::state::{
    AwaitedStart, PlayerAction, PlayerState, PlayerStateHandle, lock_state, stop_end_of_queue,
    with_state_emit,
};
use melodia_playback::player::playback::replaygain::TrackReplayGain;

/// Runs `actions` after the state lock drops. A start that fails auto-skips, so a stale
/// double-click inside the watcher's debounce doesn't dead-end playback; the track stays queued
/// until `tasks::queue_prune` removes it.
pub fn execute_actions<B: PlayerBackend>(
    actions: Vec<PlayerAction>,
    engine: &B,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
) {
    run(Pending { actions: actions.into(), skipped: 0 }, engine, player_state, sinks);
}

/// [`execute_actions`]' loop, for a batch that may carry on a skip count from an earlier one.
fn run<B: PlayerBackend>(
    mut pending: Pending,
    engine: &B,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
) {
    while let Some(action) = pending.actions.pop_front() {
        // Per action is fine: nothing periodic reaches here.
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
                // Already advanced onto this track, so a failure's skip lands on the one after,
                // stepping off a `RepeatMode::One` track too.
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
                // Only ever clears a stale preload; the monitor stages the real one.
                engine.preload_gapless(path.as_deref(), TrackReplayGain::default());
            }
            PlayerAction::PlayStream { generation, volume } => {
                // Not `start_or_skip`: no file to pre-flight, and a station has no next.
                if let Err(e) = engine.play_stream(generation, volume) {
                    log::error!("Failed to start the radio stream: {}", describe(&e));
                    engine.stop();
                    enqueue_station_failure(&mut pending, player_state, sinks, generation);
                }
            }
            // No-ops until `boot::tasks` installs the flusher; a `DbPool` here would pin the
            // engine to sqlx.
            PlayerAction::UpdatePlayCount(track_id) => {
                try_send(PlayCountEvent::Play(track_id));
            }
            PlayerAction::UpdateSkipCount(track_id) => {
                try_send(PlayCountEvent::Skip(track_id));
            }
            // Replaced rather than sent: a watch with no subscriber keeps nothing on `send`.
            PlayerAction::StartWhenResolved(awaited) => {
                sinks.awaited_start.send_replace(Some(awaited));
            }
        }
    }
}

/// Runs the start `awaited` owes once the window task has its record, unless something started
/// or stopped since. Probed before the exec lock, since the task asks after every fetch, and again
/// under it. The deferral's skip count carries on, so the lap guard still holds.
pub fn start_awaited<B: PlayerBackend>(
    engine: &B,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    awaited: AwaitedStart,
) {
    if !lock_state(player_state).still_awaits() {
        return;
    }
    let _exec = player_state.lock_exec();
    if !lock_state(player_state).still_awaits() {
        return;
    }
    let actions = with_state_emit(player_state, sinks, |s| s.resume_awaited(awaited));
    run(Pending { actions: actions.into(), skipped: awaited.skipped }, engine, player_state, sinks);
}

/// Mutates, publishes and runs the side effects under the exec lock, so mutation order is
/// side-effect order across workers: two batches from different tasks would otherwise interleave
/// and leave state and backend disagreeing. An auto-skip's nested `with_state_emit` takes the
/// state mutex, never `exec_lock`, so nothing re-enters.
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
    /// A repeating queue wraps on a skip, so without a bound one with nothing playable skips
    /// forever, holding the lock every control waits on.
    skipped: usize,
}

impl Pending {
    /// Runs `actions` next, ahead of anything still queued from the batch.
    fn prepend(&mut self, actions: Vec<PlayerAction>) {
        for (i, a) in actions.into_iter().enumerate() {
            self.actions.insert(i, a);
        }
    }
}

#[derive(Copy, Clone)]
enum StartMode {
    /// Takes over the decks; a failure leaves them half-set, so it stops before skipping on.
    Fresh,
    /// Overlaps the outgoing track, which stays audible on a failure: the skip's `play_media`
    /// takes over from it either way, and a stop would only add silence.
    Crossfade,
}

impl StartMode {
    fn stops_on_failure(self) -> bool {
        matches!(self, Self::Fresh)
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Fresh => "play",
            Self::Crossfade => "crossfade into",
        }
    }

    fn at(self) -> &'static str {
        match self {
            Self::Fresh => "playback",
            Self::Crossfade => "crossfade",
        }
    }
}

/// Starts a track, auto-skipping one that can't play. A vanished file skips silently while the
/// skip finds something playable; a decode failure always toasts, or the music stopping goes
/// unexplained.
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

/// The file name alone, the full path being too long to read in a toast.
fn toast_track_name(file_path: &str) -> String {
    Path::new(file_path)
        .file_name()
        .map_or_else(|| file_path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Clears a station whose stage couldn't start, so the transport doesn't sit on `Loading`. No
/// toast: only a stage never made, or one for a station already left, gets here.
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

/// Advances past an unplayable track, or gives up (`true`) once the batch has skipped a whole lap
/// of the queue.
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
        match s.queue.advance_skip() {
            Some(id) => s.start_queued(id, false, skipped),
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
