use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::player::engine::actions::{emit_and_execute, execute_actions, start_awaited};
use crate::player::engine::backend::PlayerBackend;
use crate::player::engine::event_sink::{MediaControlsSync, PlayerSinks};
use crate::player::engine::fixtures::test_track;
use crate::player::engine::state::{
    AwaitedStart, PlayerAction, PlayerState, PlayerStateHandle, PlayerViewModelLight, lock_state,
    with_state_emit,
};
use crate::player::engine::types::{PlaybackStatus, RepeatMode};
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_playback::player::playback::replaygain::TrackReplayGain;

/// Records all `PlayerBackend` calls for assertion in tests.
#[derive(Debug, Default)]
struct MockBackendInner {
    play_media_calls: Vec<(String, f64, f64, Option<u64>)>,
    /// `(file_path, fade_ms, volume, speed)` per `begin_crossfade`.
    begin_crossfade_calls: Vec<(String, u64, f64, f64)>,
    resume_count: u32,
    /// `fade_ms` per `pause_with_fade`, including the `0`s that mean "immediate".
    /// `pause_with_fade` is the only pause on the trait, so this is also the
    /// pause count — unlike `stop_count`, which has to cover the bare `stop()`
    /// the auto-skip path calls outside any `PlayerAction`.
    pause_fade_calls: Vec<u64>,
    stop_count: u32,
    /// `fade_ms` per `stop_with_fade`, including the `0`s that mean "immediate".
    stop_fade_calls: Vec<u64>,
    seek_calls: Vec<(String, u64)>,
    volume_calls: Vec<f64>,
    speed_calls: Vec<f64>,
    preload_calls: Vec<Option<String>>,
    /// `(generation, volume)` per `play_stream`. No path: the stream was opened long before the
    /// action ran, and the backend finds it by generation alone.
    play_stream_calls: Vec<(u64, f64)>,
    play_media_should_fail: bool,
    begin_crossfade_should_fail: bool,
    play_stream_should_fail: bool,
}

struct MockBackend {
    inner: Mutex<MockBackendInner>,
}

impl MockBackend {
    fn new() -> Self {
        Self { inner: Mutex::new(MockBackendInner::default()) }
    }

    fn with_play_failure() -> Self {
        Self {
            inner: Mutex::new(MockBackendInner {
                play_media_should_fail: true,
                ..Default::default()
            }),
        }
    }

    fn with_crossfade_failure() -> Self {
        Self {
            inner: Mutex::new(MockBackendInner {
                begin_crossfade_should_fail: true,
                ..Default::default()
            }),
        }
    }

    fn with_stream_failure() -> Self {
        Self {
            inner: Mutex::new(MockBackendInner {
                play_stream_should_fail: true,
                ..Default::default()
            }),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, MockBackendInner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl PlayerBackend for MockBackend {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        _baked_rg: TrackReplayGain,
    ) -> Result<(), AppError> {
        let mut inner = self.inner();
        if inner.play_media_should_fail {
            return Err(AppError::player_msg("mock failure"));
        }
        inner.play_media_calls.push((file_path.to_owned(), volume, speed, start_position_ms));
        Ok(())
    }

    fn begin_crossfade(
        &self,
        file_path: &str,
        _baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError> {
        let mut inner = self.inner();
        if inner.begin_crossfade_should_fail {
            return Err(AppError::player_msg("mock crossfade failure"));
        }
        inner.begin_crossfade_calls.push((file_path.to_owned(), fade_ms, volume, speed));
        Ok(())
    }

    fn resume(&self) {
        self.inner().resume_count += 1;
    }
    fn pause_with_fade(&self, fade_ms: u64) {
        self.inner().pause_fade_calls.push(fade_ms);
    }
    fn stop(&self) {
        self.inner().stop_count += 1;
    }
    fn stop_with_fade(&self, fade_ms: u64) {
        let mut inner = self.inner();
        inner.stop_fade_calls.push(fade_ms);
        inner.stop_count += 1;
    }
    fn seek(&self, file_path: &str, position_ms: u64, _baked_rg: TrackReplayGain) {
        self.inner().seek_calls.push((file_path.to_owned(), position_ms));
    }
    fn set_volume(&self, volume: f64) {
        self.inner().volume_calls.push(volume);
    }
    fn set_speed(&self, speed: f64) {
        self.inner().speed_calls.push(speed);
    }
    fn preload_gapless(&self, file_path: Option<&str>, _baked_rg: TrackReplayGain) {
        self.inner().preload_calls.push(file_path.map(std::borrow::ToOwned::to_owned));
    }
    fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError> {
        let mut inner = self.inner();
        if inner.play_stream_should_fail {
            return Err(AppError::player_msg("mock stream failure"));
        }
        inner.play_stream_calls.push((generation, volume));
        Ok(())
    }
}

/// Minimal `PlayerSinks` for tests — no media-controls sync, watch channels
/// with no live consumers. `execute_actions` only writes to these, so the
/// dropped receivers don't matter.
fn make_test_sinks() -> PlayerSinks {
    let (view_model, _) = watch::channel(None);
    let (queue, _) = watch::channel(None);
    let (awaited_start, _) = watch::channel(None);
    PlayerSinks { view_model, queue, media_controls: None, awaited_start }
}

/// Bundles every fixture the `execute_actions` tests need. The temp dir is
/// kept alive for the duration of the test so writers can flush to disk
/// without racing the directory's drop. A real, zero-byte audio file lives
/// at `track_path` so the `Path::exists()` pre-flight check in
/// `execute_actions` lets `PlayMedia` reach the (mock) backend.
struct ActionsFixture {
    player_state: PlayerStateHandle,
    sinks: PlayerSinks,
    track_path: String,
    /// A second real file, for a test that starts two tracks and has to tell them apart.
    next_path: String,
    _tmp: tempfile::TempDir,
}

fn fixture() -> Result<ActionsFixture, AppError> {
    let tmp = tempfile::tempdir()?;
    let track_path = tmp.path().join("track.mp3");
    let next_path = tmp.path().join("next.mp3");
    std::fs::write(&track_path, b"")?;
    std::fs::write(&next_path, b"")?;
    Ok(ActionsFixture {
        player_state: PlayerStateHandle::default(),
        sinks: make_test_sinks(),
        track_path: track_path.to_string_lossy().into_owned(),
        next_path: next_path.to_string_lossy().into_owned(),
        _tmp: tmp,
    })
}

#[tokio::test]
async fn execute_play_media_calls_backend() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::PlayMedia {
        file_path: fx.track_path.clone(),
        volume: 0.8,
        speed: 1.0,
        start_position_ms: Some(5000),
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.play_media_calls.len(), 1);
    assert_eq!(inner.play_media_calls[0].0, fx.track_path);
    assert!((inner.play_media_calls[0].1 - 0.8).abs() < f64::EPSILON);
    assert!((inner.play_media_calls[0].2 - 1.0).abs() < f64::EPSILON);
    assert_eq!(inner.play_media_calls[0].3, Some(5000));
    Ok(())
}

#[tokio::test]
async fn execute_play_media_decode_failure_triggers_auto_skip() -> Result<(), AppError> {
    // The track file exists on disk, so pre-flight lets the action through;
    // the mock backend then returns Err to simulate a decode failure. With
    // an empty queue, auto-skip lands on `stop_end_of_queue`, so stop is
    // called twice — once for the failure handling, once for the empty-queue
    // skip.
    let fx = fixture()?;
    let mock = MockBackend::with_play_failure();

    let actions = vec![PlayerAction::PlayMedia {
        file_path: fx.track_path.clone(),
        volume: 0.5,
        speed: 1.0,
        start_position_ms: None,
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.play_media_calls.is_empty());
    assert_eq!(inner.stop_count, 2);
    Ok(())
}

#[tokio::test]
async fn execute_play_media_vanished_file_pre_flights() -> Result<(), AppError> {
    // Pre-flight catches the missing file before any decode attempt: the
    // mock's `play_media` is never invoked, and auto-skip stops the empty
    // queue.
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::PlayMedia {
        file_path: "/nonexistent/track.mp3".to_owned(),
        volume: 1.0,
        speed: 1.0,
        start_position_ms: None,
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.play_media_calls.is_empty());
    assert_eq!(inner.stop_count, 2);
    Ok(())
}

#[tokio::test]
async fn execute_resume_pause_stop() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![
        PlayerAction::Resume,
        PlayerAction::Pause { fade_ms: 0 },
        PlayerAction::Stop { fade_ms: 0 },
    ];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.resume_count, 1);
    assert_eq!(inner.pause_fade_calls, vec![0]);
    assert_eq!(inner.stop_count, 1);
    Ok(())
}

#[tokio::test]
async fn execute_stop_forwards_the_fade_length() -> Result<(), AppError> {
    // A user stop with fade-on-pause carries the ramp length; the end-of-queue
    // stop passes 0 so it lands immediately.
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::Stop { fade_ms: 250 }, PlayerAction::Stop { fade_ms: 0 }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    assert_eq!(mock.inner().stop_fade_calls, vec![250, 0]);
    Ok(())
}

#[tokio::test]
async fn execute_pause_forwards_the_fade_length() -> Result<(), AppError> {
    // A user pause with fade-on-pause carries the ramp length; the `Pause` that
    // next/previous append to restore a paused deck passes 0, so the incoming
    // track can never be heard fading out on arrival.
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::Pause { fade_ms: 250 }, PlayerAction::Pause { fade_ms: 0 }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    assert_eq!(mock.inner().pause_fade_calls, vec![250, 0]);
    Ok(())
}

#[tokio::test]
async fn execute_begin_crossfade_calls_backend() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::BeginCrossfade {
        file_path: fx.track_path.clone(),
        replaygain: TrackReplayGain::default(),
        fade_ms: 1_800,
        volume: 0.8,
        speed: 1.0,
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.begin_crossfade_calls.len(), 1);
    assert_eq!(inner.begin_crossfade_calls[0].0, fx.track_path);
    assert_eq!(inner.begin_crossfade_calls[0].1, 1_800);
    assert!((inner.begin_crossfade_calls[0].2 - 0.8).abs() < f64::EPSILON);
    assert_eq!(inner.stop_count, 0, "a successful crossfade must not stop the decks");
    Ok(())
}

#[tokio::test]
async fn execute_begin_crossfade_vanished_file_skips_without_stopping() -> Result<(), AppError> {
    // The outgoing track is still audible on the other deck, so the crossfade
    // arm must NOT stop the player the way `PlayMedia` does — the auto-skip's
    // `play_media` clears both decks anyway. With an empty queue the skip lands
    // on `stop_end_of_queue`, which is the single stop we expect.
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::BeginCrossfade {
        file_path: "/nonexistent/track.mp3".to_owned(),
        replaygain: TrackReplayGain::default(),
        fade_ms: 2_000,
        volume: 1.0,
        speed: 1.0,
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.begin_crossfade_calls.is_empty());
    assert_eq!(inner.stop_count, 1, "only the empty-queue auto-skip may stop");
    Ok(())
}

#[tokio::test]
async fn execute_begin_crossfade_decode_failure_triggers_auto_skip() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::with_crossfade_failure();

    let actions = vec![PlayerAction::BeginCrossfade {
        file_path: fx.track_path.clone(),
        replaygain: TrackReplayGain::default(),
        fade_ms: 2_000,
        volume: 1.0,
        speed: 1.0,
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.begin_crossfade_calls.is_empty());
    assert_eq!(inner.stop_count, 1, "only the empty-queue auto-skip may stop");
    Ok(())
}

#[tokio::test]
async fn execute_seek_calls_backend() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::Seek {
        position_ms: 30_000,
        file_path: "/music/a.flac".to_owned(),
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.seek_calls, vec![("/music/a.flac".to_owned(), 30_000)]);
    Ok(())
}

/// The seeks the OS media controls were told about. `sync` records nothing: it belongs to the
/// emit before `execute_actions`, so only what this layer sends itself is worth watching.
#[derive(Default)]
struct SeekRecorder {
    seeks: Mutex<Vec<u64>>,
}

impl SeekRecorder {
    fn seeks(&self) -> Vec<u64> {
        self.seeks.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl MediaControlsSync for SeekRecorder {
    fn sync(&self, _vm: &PlayerViewModelLight, _status: PlaybackStatus) {}

    fn seeked(&self, position_ms: u64) {
        self.seeks.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(position_ms);
    }
}

/// An MPRIS client advances its bar on its own between reads and learns of a jump only through
/// `Seeked`, so a seek that told nobody leaves an open panel drawing the old position.
#[test]
fn a_seek_tells_the_media_controls_where_it_landed() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();
    let recorder = Arc::new(SeekRecorder::default());
    let media_controls: Arc<dyn MediaControlsSync> = recorder.clone();
    let sinks = PlayerSinks { media_controls: Some(media_controls), ..make_test_sinks() };
    let actions = vec![PlayerAction::Seek {
        position_ms: 30_000,
        file_path: "/music/a.flac".to_owned(),
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &sinks);

    assert_eq!(recorder.seeks(), vec![30_000]);
    Ok(())
}

#[tokio::test]
async fn execute_set_volume_and_speed() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::SetVolume(0.75), PlayerAction::SetSpeed(1.5)];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.volume_calls, vec![0.75]);
    assert_eq!(inner.speed_calls, vec![1.5]);
    Ok(())
}

#[tokio::test]
async fn execute_clear_gapless_preload_stages_nothing() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::ClearGaplessPreload];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    assert_eq!(mock.inner().preload_calls, vec![None]);
    Ok(())
}

#[tokio::test]
async fn execute_multiple_actions_in_order() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    let actions = vec![
        PlayerAction::PlayMedia {
            file_path: fx.track_path.clone(),
            volume: 1.0,
            speed: 1.0,
            start_position_ms: None,
            replaygain: TrackReplayGain::default(),
        },
        PlayerAction::ClearGaplessPreload,
        PlayerAction::SetVolume(0.5),
    ];

    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.play_media_calls.len(), 1);
    assert_eq!(inner.preload_calls.len(), 1);
    assert_eq!(inner.volume_calls, vec![0.5]);
    Ok(())
}

#[tokio::test]
async fn execute_empty_actions_is_noop() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();

    crate::player::engine::actions::execute_actions(vec![], &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.play_media_calls.is_empty());
    assert_eq!(inner.resume_count, 0);
    assert_eq!(inner.stop_count, 0);
    Ok(())
}

// --- Live streams ----------------------------------------------------------

/// Tune the fixture's state to a station and hand back the session generation.
fn tune_in(player_state: &PlayerStateHandle) -> u64 {
    let mut state = crate::player::engine::state::lock_state(player_state);
    let (generation, _actions) = state.build_station_connecting_actions(
        crate::player::engine::fixtures::test_station("Example FM"),
    );
    generation
}

/// The stream was opened long before the action ran, so there is no path to pre-flight with
/// `Path::exists` and nothing to auto-skip onto — a station is one source, not a queue position.
#[tokio::test]
async fn execute_play_stream_reaches_the_backend_without_a_path() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();
    let generation = tune_in(&fx.player_state);

    let actions = vec![PlayerAction::PlayStream { generation, volume: 0.8 }];
    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.play_stream_calls.len(), 1);
    assert_eq!(inner.play_stream_calls[0].0, generation);
    assert!((inner.play_stream_calls[0].1 - 0.8).abs() < f64::EPSILON);
    assert_eq!(inner.stop_count, 0, "a successful start stops nothing");
    Ok(())
}

#[tokio::test]
async fn execute_play_stream_failure_clears_the_station() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::with_stream_failure();
    let generation = tune_in(&fx.player_state);

    let actions = vec![PlayerAction::PlayStream { generation, volume: 1.0 }];
    crate::player::engine::actions::execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let state = crate::player::engine::state::lock_state(&fx.player_state);
    assert!(state.station().is_none(), "a station that could not start must not sit on Loading");
    assert_eq!(state.status, crate::player::engine::types::PlaybackStatus::Stopped);
    // The failure path is `build_station_failed_actions`, not `enqueue_auto_skip`: there is no
    // next station to fall through to, and reaching into the queue would be a change of source.
    assert!(mock.inner().play_media_calls.is_empty());
    Ok(())
}

/// The records of `paths`, their ids counting from 1 in order.
fn records_for(paths: &[&str]) -> Vec<Arc<TrackSummary>> {
    paths
        .iter()
        .zip(1..)
        .map(|(path, id)| {
            let mut track = (*test_track("t", None, None)).clone();
            track.id = id;
            (*path).clone_into(&mut track.file_path);
            Arc::new(track)
        })
        .collect()
}

/// Queues `paths` under `mode`, every record in the window, and returns the actions that start the
/// first of them.
fn start_queue(fx: &ActionsFixture, paths: &[&str], mode: RepeatMode) -> Vec<PlayerAction> {
    start_queue_holding(fx, paths, mode, paths.len())
}

/// [`start_queue`] with only the first `held` records in the window, the rest yet to be fetched.
fn start_queue_holding(
    fx: &ActionsFixture,
    paths: &[&str],
    mode: RepeatMode,
    held: usize,
) -> Vec<PlayerAction> {
    let tracks = records_for(paths);
    let ids: Vec<i64> = tracks.iter().map(|track| track.id).collect();
    with_state_emit(&fx.player_state, &fx.sinks, |s| {
        s.queue.add_tracks(&ids);
        s.queue.set_repeat_mode(mode);
        s.window.seat(&ids, tracks.into_iter().take(held));
        match s.queue.skip_to_index(0) {
            Some(first) => s.start_queued(first, false, 0),
            None => Vec::new(),
        }
    })
}

/// The window task landing the record of every one of `paths` the window still wants.
fn land_every_record(fx: &ActionsFixture, paths: &[&str]) {
    let mut state = lock_state(&fx.player_state);
    let wanted = state.queue.window_ids();
    state.window.seat(&wanted, records_for(paths));
}

/// The start the transition last owed, as the window task finds it.
fn owed(fx: &ActionsFixture) -> Result<AwaitedStart, AppError> {
    (*fx.sinks.awaited_start.borrow())
        .ok_or_else(|| AppError::validation_msg("no transition owed a start"))
}

/// The fixture's two files queued and the first playing, then a Next onto the second, whose record
/// the window lacks.
fn skipped_onto_an_unfetched_track(fx: &ActionsFixture, mock: &MockBackend) {
    let paths = [fx.track_path.as_str(), fx.next_path.as_str()];
    let started = start_queue_holding(fx, &paths, RepeatMode::Off, 1);
    execute_actions(started, mock, &fx.player_state, &fx.sinks);
    emit_and_execute(mock, &fx.player_state, &fx.sinks, PlayerState::build_next_actions);
}

/// The window task resolves an owed start off this cell, and nothing may be subscribed the moment
/// it is owed: a `send` with no receiver keeps nothing, which would leave the start owed for good.
#[test]
fn an_owed_start_is_kept_for_the_window_task_with_nobody_subscribed() -> Result<(), AppError> {
    let fx = fixture()?;
    let start = AwaitedStart { id: 2, re_pause: false, skipped: 0 };

    execute_actions(
        vec![PlayerAction::StartWhenResolved(start)],
        &MockBackend::new(),
        &fx.player_state,
        &fx.sinks,
    );

    assert_eq!(*fx.sinks.awaited_start.borrow(), Some(start));
    Ok(())
}

/// Once its record lands, the start a Next owed reaches the backend as the track the queue is on.
#[test]
fn a_deferred_start_reaches_the_backend_once_its_record_lands() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();
    skipped_onto_an_unfetched_track(&fx, &mock);
    let start = owed(&fx)?;
    land_every_record(&fx, &[fx.track_path.as_str(), fx.next_path.as_str()]);

    start_awaited(&mock, &fx.player_state, &fx.sinks, start);

    let played: Vec<String> =
        mock.inner().play_media_calls.iter().map(|call| call.0.clone()).collect();
    assert_eq!(played, [fx.track_path.clone(), fx.next_path.clone()]);
    Ok(())
}

/// The window task asks after every fetch, whatever woke it, so a start still waiting on its
/// record is turned away for the price of a lock. Through the emit it would republish the view
/// model on every wake.
#[test]
fn a_deferred_start_still_waiting_on_its_record_publishes_nothing() -> Result<(), AppError> {
    let fx = fixture()?;
    let mock = MockBackend::new();
    skipped_onto_an_unfetched_track(&fx, &mock);
    let start = owed(&fx)?;
    let published = fx.sinks.view_model.subscribe();

    start_awaited(&mock, &fx.player_state, &fx.sinks, start);

    assert_eq!(published.has_changed().ok(), Some(false));
    Ok(())
}

/// The lap guard counts what a batch has skipped, and a deferral splits the batch in two. The half
/// run once the record lands carries the count on, or a queue with nothing playable in it plays a
/// lap and a half before it stops.
#[test]
fn a_skip_count_carries_across_a_deferral() -> Result<(), AppError> {
    let fx = fixture()?;
    let paths = [
        "/nonexistent/a.flac",
        "/nonexistent/b.flac",
        "/nonexistent/c.flac",
        "/nonexistent/d.flac",
    ];
    // a and b are skipped from the records in hand, and the skip onto c defers.
    let started = start_queue_holding(&fx, &paths, RepeatMode::All, 2);
    execute_actions(started, &MockBackend::new(), &fx.player_state, &fx.sinks);
    let start = owed(&fx)?;
    land_every_record(&fx, &paths);

    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        start_awaited(&MockBackend::new(), &fx.player_state, &fx.sinks, start);
        let _ = done.send(lock_state(&fx.player_state).queue.current_index);
    });
    let stopped_at = finished.recv_timeout(Duration::from_secs(5));

    assert_eq!(stopped_at.ok(), Some(Some(3)), "a lap from a ends on d");
    Ok(())
}

/// A repeating queue wraps on a skip, so with nothing playable in it the skip chain has no end of
/// its own. It held the lock every transport control waits on, so Pause did nothing and the log
/// filled until the app was killed.
#[test]
fn a_repeating_queue_with_nothing_playable_stops_after_one_lap() -> Result<(), AppError> {
    let fx = fixture()?;
    let actions = start_queue(
        &fx,
        &["/nonexistent/a.flac", "/nonexistent/b.flac", "/nonexistent/c.flac"],
        RepeatMode::All,
    );

    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mock = MockBackend::new();
        execute_actions(actions, &mock, &fx.player_state, &fx.sinks);
        let status = lock_state(&fx.player_state).status;
        let _ = done.send((status, mock.inner().play_media_calls.len()));
    });
    let outcome = finished.recv_timeout(Duration::from_secs(5));

    assert!(
        matches!(outcome, Ok((PlaybackStatus::Stopped, 0))),
        "expected a stop with nothing started, got {outcome:?}"
    );
    Ok(())
}

/// The bound is a lap, not a count of failures: missing tracks ahead of a playable one are
/// skipped past rather than giving up on it.
#[test]
fn a_playable_track_behind_missing_ones_still_plays() -> Result<(), AppError> {
    let fx = fixture()?;
    let playable = fx.track_path.clone();
    let actions = start_queue(
        &fx,
        &["/nonexistent/a.flac", "/nonexistent/b.flac", &playable],
        RepeatMode::One,
    );
    let mock = MockBackend::new();

    execute_actions(actions, &mock, &fx.player_state, &fx.sinks);

    let played: Vec<String> =
        mock.inner().play_media_calls.iter().map(|call| call.0.clone()).collect();
    assert_eq!(played, [playable]);
    Ok(())
}
