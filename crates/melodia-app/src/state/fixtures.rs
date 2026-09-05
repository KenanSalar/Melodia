//! A [`PlaybackContext`] the library suites can build, with no audio device under it.
//!
//! `#[cfg(test)] pub(crate)` rather than the `#[doc(hidden)] pub` its two namesakes in
//! `melodia-store` and `melodia-engine` carry: those are visible because a `cfg(test)` item cannot
//! cross a crate boundary and each has a reader on the other side of one. This has none.
//!
//! `AppState::init` opens the card, so nothing below `headless.rs` can build the real thing. What
//! `PlaybackContext` needs is a `PlaybackEngine`, and `output::mixer::pair` builds a mixer with no
//! device between its halves, which is the same seam `crates/melodia/tests/crossfade.rs` drives
//! the whole DSP chain through.
//!
//! **A thread stands in for the audio callback**, because half the transport waits on one:
//! `Voice::clear` blocks until its command is serviced, so a second `play_media` or any stop over
//! a deck that holds a source would spend `output::voice::SERVICE_TIMEOUT` against a mixer nobody
//! pulls. Pulling also drains what is playing, which no assertion here reads: `PlayerState` is
//! what these suites are about, and the audio thread never touches it.

use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::watch;

use super::PlaybackContext;
use melodia_audio::player::source::audio::{Sample, Shape};
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_engine::player::engine::backend::PlaybackEngine;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::PlayerStateHandle;
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::mixer;
use melodia_store::database::DbPool;

/// The shape the device-free mixer is brought to. Nothing listens, so these only have to be a
/// shape a `Converter` accepts and a decoder can be resampled to.
const TEST_RATE: u32 = 44_100;
const TEST_CHANNELS: u16 = 2;

/// Frames the stand-in callback asks for per pass. A real host period, so a source retires in a
/// plausible number of passes rather than in one.
const BLOCK_FRAMES: usize = 1024;

/// How long the stand-in callback idles between passes. Well inside `SERVICE_TIMEOUT`, which is
/// what a control op waiting on it has to land in.
const CALLBACK_IDLE: Duration = Duration::from_millis(1);

/// The publish half `with_state_emit` takes, for a body narrowed far enough not to need an engine
/// under it. Nothing subscribes unless the caller does; a `watch` send with no receivers is a
/// no-op, which is what these two channels are here to be until one asks.
pub(crate) fn test_sinks() -> PlayerSinks {
    let (view_model, _) = watch::channel(None);
    let (queue, _) = watch::channel(None);
    PlayerSinks {
        view_model,
        queue,
        media_controls: None,
    }
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

        // The floors are unreachable, both arguments being literals, and they are what keeps the
        // fixture clear of an `unwrap` the tree does not allow anywhere.
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
}

impl Drop for TestPlayback {
    fn drop(&mut self) {
        let Some(callback) = self.callback.take() else {
            return;
        };
        callback.stop.store(true, Ordering::Relaxed);
        // A thread that panicked has already failed the test through whatever it was servicing;
        // there is nothing useful to say about it from a destructor.
        let _ = callback.thread.join();
    }
}
