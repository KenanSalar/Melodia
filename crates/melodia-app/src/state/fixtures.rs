//! What the library suites build over: a data root with settings already written, and a
//! [`PlaybackContext`] with no audio device under it.
//!
//! `#[cfg(test)] pub(crate)`, unlike its namesakes in `melodia-store` and `melodia-engine`, which
//! have readers across the crate boundary a `cfg(test)` item can't cross. `AppState::init` opens
//! the card, so the engine here sits on `output::mixer::pair`, a mixer with no device between its
//! halves.
//!
//! **A thread stands in for the audio callback**, `Voice::clear` blocking until its command is
//! serviced: without one, a second `play_media` or a stop over a loaded deck would spend
//! `output::voice::SERVICE_TIMEOUT` against a mixer nobody pulls.

use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::watch;

use super::PlaybackContext;
use crate::services::settings::{SettingsData, write_settings};
use melodia_audio::player::source::audio::{Sample, Shape};
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_engine::player::engine::backend::PlaybackEngine;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::PlayerStateHandle;
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::mixer;
use melodia_store::database::queries::fixtures::insert_test_track;
use melodia_store::database::{DbPool, queries};
use melodia_testkit::ASSETS_DIR;

/// The device-free mixer's shape. Nothing listens, so any a `Converter` accepts will do.
const TEST_RATE: u32 = 44_100;
const TEST_CHANNELS: u16 = 2;

/// Frames the stand-in callback asks for per pass: a real host period.
const BLOCK_FRAMES: usize = 1024;

/// How long the stand-in callback idles between passes, well inside `SERVICE_TIMEOUT`.
const CALLBACK_IDLE: Duration = Duration::from_millis(1);

/// The publish half `with_state_emit` takes, for a body that needs no engine. A send with no
/// receivers is a no-op until a caller subscribes.
pub(crate) fn test_sinks() -> PlayerSinks {
    let (view_model, _) = watch::channel(None);
    let (queue, _) = watch::channel(None);
    let (awaited_start, _) = watch::channel(None);
    PlayerSinks { view_model, queue, media_controls: None, awaited_start }
}

/// A data root nothing else writes to, with `settings.json` already on disk.
///
/// Seeded because a missing file reads as `SettingsData::default()`, whose serde defaults reach the
/// environment the sibling suites mutate. Written once under `reading_env`, every field included,
/// so nothing later parses a default. Not callable inside a `with_env_var` body: both take the one
/// env lock, which isn't reentrant.
pub(crate) fn seeded_root() -> Result<(TempDir, Paths), AppError> {
    seeded_root_with(|_| {})
}

/// [`seeded_root`] with `mutate` applied to what it seeds, for a suite whose first call has to
/// read a value an earlier launch left behind.
pub(crate) fn seeded_root_with(
    mutate: impl FnOnce(&mut SettingsData),
) -> Result<(TempDir, Paths), AppError> {
    let tmp = TempDir::new()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;

    let mut settings = melodia_testkit::reading_env(SettingsData::default);
    mutate(&mut settings);
    write_settings(&paths, &settings)?;
    Ok((tmp, paths))
}

pub(crate) struct TestPlayback {
    pub(crate) ctx: PlaybackContext,
    /// The data root `ctx.paths` names, kept for the settings and queue files written under it.
    pub(crate) tmp: TempDir,
    callback: Option<Callback>,
}

/// The thread pulling the mixer, and the flag that ends it.
struct Callback {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl TestPlayback {
    /// Build a context over `db`. Must be called from inside a tokio runtime: the engine keeps a
    /// [`tokio::runtime::Handle`] for the deferred half of a faded pause.
    pub(crate) fn with_db(db: DbPool) -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let paths = Paths::rooted_at(tmp.path().to_path_buf());
        paths.create_dirs()?;

        // Unreachable floors, both arguments being literals, in place of an `unwrap`.
        let device = Shape {
            channels: NonZero::new(TEST_CHANNELS).unwrap_or(NonZero::<u16>::MIN),
            rate: NonZero::new(TEST_RATE).unwrap_or(NonZero::<u32>::MIN),
        };
        let (mixer, mut pull) = mixer::pair(DECK_COUNT, device);
        let engine = PlaybackEngine::new(&mixer, tokio::runtime::Handle::current())?;

        let stop = Arc::new(AtomicBool::new(false));
        let ending = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut block: Vec<Sample> = vec![0.0; BLOCK_FRAMES * usize::from(TEST_CHANNELS)];
            while !ending.load(Ordering::Relaxed) {
                pull.fill(&mut block);
                std::thread::sleep(CALLBACK_IDLE);
            }
        });

        Ok(Self {
            ctx: PlaybackContext {
                player_state: Arc::new(PlayerStateHandle::default()),
                sinks: Arc::new(test_sinks()),
                engine: Arc::new(engine),
                db,
                paths: Arc::new(paths),
                http: Arc::new(OnceLock::new()),
            },
            tmp,
            callback: Some(Callback { stop, thread }),
        })
    }

    /// A context over an empty in-memory library, for the commands that never read one.
    pub(crate) async fn empty() -> Result<Self, AppError> {
        Self::with_db(DbPool::test_pool().await?)
    }

    /// `count` playable files under the fixture's root, with rows pointing at them, returning
    /// their ids in queue order. Real copies of `silence.mp3`, because `execute_actions` skips a
    /// `PlayMedia` whose file isn't there and a queue of missing files walks itself to the end.
    pub(crate) async fn stage_playable(&self, count: usize) -> Result<Vec<i64>, AppError> {
        let dir = self.tmp.path().join("music");
        std::fs::create_dir_all(&dir)?;
        queries::folder::insert_folder(&self.ctx.db, &dir.to_string_lossy(), true).await?;

        let silence = std::path::PathBuf::from(ASSETS_DIR).join("silence.mp3");
        let mut ids = Vec::with_capacity(count);
        for n in 1..=count {
            let dest = dir.join(format!("track{n}.mp3"));
            std::fs::copy(&silence, &dest)?;
            let title = format!("Track {n}");
            ids.push(
                insert_test_track(
                    &self.ctx.db,
                    &dest.to_string_lossy(),
                    &title,
                    "Artist",
                    "Album",
                    "Rock",
                )
                .await?,
            );
        }
        Ok(ids)
    }
}

impl Drop for TestPlayback {
    fn drop(&mut self) {
        let Some(callback) = self.callback.take() else {
            return;
        };
        callback.stop.store(true, Ordering::Relaxed);
        // A panicked thread has already failed the test through whatever it was servicing.
        let _ = callback.thread.join();
    }
}
