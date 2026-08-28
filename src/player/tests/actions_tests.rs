use std::sync::Mutex;

use tokio::sync::watch;

use crate::error::AppError;
use crate::player::event_sink::PlayerSinks;
use crate::player::replaygain::TrackReplayGain;
use crate::player::rodio_backend::PlayerBackend;
use crate::player::state::{PlayerAction, PlayerStateHandle};

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
    seek_calls: Vec<u64>,
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
        Self {
            inner: Mutex::new(MockBackendInner::default()),
        }
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
            return Err(AppError::Player("mock failure".to_owned()));
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
            return Err(AppError::Player("mock crossfade failure".to_owned()));
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
    fn seek(&self, position_ms: u64) {
        self.inner().seek_calls.push(position_ms);
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
            return Err(AppError::Player("mock stream failure".to_owned()));
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
    PlayerSinks {
        view_model,
        queue,
        media_controls: None,
    }
}

/// Bundles every fixture the `execute_actions` tests need. The temp dir is
/// kept alive for the duration of the test so writers can flush to disk
/// without racing the directory's drop. A real, zero-byte audio file lives
/// at `track_path` so the `Path::exists()` pre-flight check in
/// `execute_actions` lets `PlayMedia` reach the (mock) backend.
struct ActionsFixture {
    db: crate::database::DbPool,
    player_state: PlayerStateHandle,
    sinks: PlayerSinks,
    track_path: String,
    _tmp: tempfile::TempDir,
}

async fn fixture() -> Result<ActionsFixture, AppError> {
    let tmp = tempfile::tempdir()?;
    let track_path = tmp.path().join("track.mp3");
    std::fs::write(&track_path, b"")?;
    let db = crate::database::DbPool::test_pool().await?;
    Ok(ActionsFixture {
        db,
        player_state: PlayerStateHandle::default(),
        sinks: make_test_sinks(),
        track_path: track_path.to_string_lossy().into_owned(),
        _tmp: tmp,
    })
}

#[tokio::test]
async fn execute_play_media_calls_backend() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::PlayMedia {
        file_path: fx.track_path.clone(),
        volume: 0.8,
        speed: 1.0,
        start_position_ms: Some(5000),
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

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
    let fx = fixture().await?;
    let mock = MockBackend::with_play_failure();

    let actions = vec![PlayerAction::PlayMedia {
        file_path: fx.track_path.clone(),
        volume: 0.5,
        speed: 1.0,
        start_position_ms: None,
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

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
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::PlayMedia {
        file_path: "/nonexistent/track.mp3".to_owned(),
        volume: 1.0,
        speed: 1.0,
        start_position_ms: None,
        replaygain: TrackReplayGain::default(),
    }];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.play_media_calls.is_empty());
    assert_eq!(inner.stop_count, 2);
    Ok(())
}

#[tokio::test]
async fn execute_resume_pause_stop() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![
        PlayerAction::Resume,
        PlayerAction::Pause { fade_ms: 0 },
        PlayerAction::Stop { fade_ms: 0 },
    ];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

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
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![
        PlayerAction::Stop { fade_ms: 250 },
        PlayerAction::Stop { fade_ms: 0 },
    ];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    assert_eq!(mock.inner().stop_fade_calls, vec![250, 0]);
    Ok(())
}

#[tokio::test]
async fn execute_pause_forwards_the_fade_length() -> Result<(), AppError> {
    // A user pause with fade-on-pause carries the ramp length; the `Pause` that
    // next/previous append to restore a paused deck passes 0, so the incoming
    // track can never be heard fading out on arrival.
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![
        PlayerAction::Pause { fade_ms: 250 },
        PlayerAction::Pause { fade_ms: 0 },
    ];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    assert_eq!(mock.inner().pause_fade_calls, vec![250, 0]);
    Ok(())
}

#[tokio::test]
async fn execute_begin_crossfade_calls_backend() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::BeginCrossfade {
        file_path: fx.track_path.clone(),
        replaygain: TrackReplayGain::default(),
        fade_ms: 1_800,
        volume: 0.8,
        speed: 1.0,
    }];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

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
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::BeginCrossfade {
        file_path: "/nonexistent/track.mp3".to_owned(),
        replaygain: TrackReplayGain::default(),
        fade_ms: 2_000,
        volume: 1.0,
        speed: 1.0,
    }];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.begin_crossfade_calls.is_empty());
    assert_eq!(inner.stop_count, 1, "only the empty-queue auto-skip may stop");
    Ok(())
}

#[tokio::test]
async fn execute_begin_crossfade_decode_failure_triggers_auto_skip() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::with_crossfade_failure();

    let actions = vec![PlayerAction::BeginCrossfade {
        file_path: fx.track_path.clone(),
        replaygain: TrackReplayGain::default(),
        fade_ms: 2_000,
        volume: 1.0,
        speed: 1.0,
    }];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.begin_crossfade_calls.is_empty());
    assert_eq!(inner.stop_count, 1, "only the empty-queue auto-skip may stop");
    Ok(())
}

#[tokio::test]
async fn execute_seek_calls_backend() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::Seek {
        position_ms: 30_000,
    }];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.seek_calls, vec![30_000]);
    Ok(())
}

#[tokio::test]
async fn execute_set_volume_and_speed() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![PlayerAction::SetVolume(0.75), PlayerAction::SetSpeed(1.5)];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.volume_calls, vec![0.75]);
    assert_eq!(inner.speed_calls, vec![1.5]);
    Ok(())
}

#[tokio::test]
async fn execute_preload_gapless_with_and_without_path() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![
        PlayerAction::PreloadGapless(Some("/music/next.mp3".to_owned())),
        PlayerAction::PreloadGapless(None),
    ];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.preload_calls.len(), 2);
    assert_eq!(inner.preload_calls[0], Some("/music/next.mp3".to_owned()));
    assert_eq!(inner.preload_calls[1], None);
    Ok(())
}

#[tokio::test]
async fn execute_multiple_actions_in_order() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    let actions = vec![
        PlayerAction::PlayMedia {
            file_path: fx.track_path.clone(),
            volume: 1.0,
            speed: 1.0,
            start_position_ms: None,
            replaygain: TrackReplayGain::default(),
        },
        PlayerAction::PreloadGapless(Some("/music/next.mp3".to_owned())),
        PlayerAction::SetVolume(0.5),
    ];

    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.play_media_calls.len(), 1);
    assert_eq!(inner.preload_calls.len(), 1);
    assert_eq!(inner.volume_calls, vec![0.5]);
    Ok(())
}

#[tokio::test]
async fn execute_empty_actions_is_noop() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();

    crate::player::actions::execute_actions(vec![], &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert!(inner.play_media_calls.is_empty());
    assert_eq!(inner.resume_count, 0);
    assert_eq!(inner.stop_count, 0);
    Ok(())
}

// --- Live streams ----------------------------------------------------------

/// Tune the fixture's state to a station and hand back the session generation.
fn tune_in(player_state: &PlayerStateHandle) -> u64 {
    let mut state = crate::player::state::lock_state(player_state);
    let (generation, _actions) = state.build_station_connecting_actions(
        crate::player::tests::helpers::test_station("Example FM"),
    );
    generation
}

/// The stream was opened long before the action ran, so there is no path to pre-flight with
/// `Path::exists` and nothing to auto-skip onto — a station is one source, not a queue position.
#[tokio::test]
async fn execute_play_stream_reaches_the_backend_without_a_path() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::new();
    let generation = tune_in(&fx.player_state);

    let actions = vec![PlayerAction::PlayStream {
        generation,
        volume: 0.8,
    }];
    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let inner = mock.inner();
    assert_eq!(inner.play_stream_calls.len(), 1);
    assert_eq!(inner.play_stream_calls[0].0, generation);
    assert!((inner.play_stream_calls[0].1 - 0.8).abs() < f64::EPSILON);
    assert_eq!(inner.stop_count, 0, "a successful start stops nothing");
    Ok(())
}

#[tokio::test]
async fn execute_play_stream_failure_clears_the_station() -> Result<(), AppError> {
    let fx = fixture().await?;
    let mock = MockBackend::with_stream_failure();
    let generation = tune_in(&fx.player_state);

    let actions = vec![PlayerAction::PlayStream {
        generation,
        volume: 1.0,
    }];
    crate::player::actions::execute_actions(actions, &mock, &fx.db, &fx.player_state, &fx.sinks);

    let state = crate::player::state::lock_state(&fx.player_state);
    assert!(state.radio.is_none(), "a station that could not start must not sit on Loading");
    assert_eq!(state.status, crate::player::types::PlaybackStatus::Stopped);
    // The failure path is `build_station_failed_actions`, not `enqueue_auto_skip`: there is no
    // next station to fall through to, and reaching into the queue would be a change of source.
    assert!(mock.inner().play_media_calls.is_empty());
    Ok(())
}
