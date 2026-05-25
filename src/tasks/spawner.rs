//! [`TaskSpawner`] — the single primitive every background task in this
//! module uses to register itself on the shared shutdown lifecycle.
//!
//! Before this lived here, every `tasks/*::spawn` reached into `AppState`
//! and pulled `task_tracker` + `shutdown_token` out by hand — some via
//! `&state.task_tracker`, some via `state.task_tracker.clone()`, some
//! via the inline `state.task_tracker.spawn(async move {})` form. Two
//! tasks (`play_count_flusher`, `media_controls::spawn_event_receiver`)
//! took the pair as raw `&TaskTracker, CancellationToken` parameters,
//! and the cancel-listen plumbing was hand-rolled in `heap_trim` /
//! `playback_monitor` / `file_event_processor`. The drift meant a new
//! task author could easily forget the `select! { _ = shutdown.cancelled()
//! => return; }` pattern and pin the runtime shutdown indefinitely.
//!
//! Use [`TaskSpawner::spawn`] for fire-and-forget work that completes on
//! its own (e.g. a `tokio::time::sleep` deadline, a channel close);
//! [`TaskSpawner::spawn_cancellable`] when the task is a `loop` /
//! `tokio::select!` that should exit on shutdown.

use std::future::Future;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::state::AppState;

/// Bundled task lifecycle primitives. Cheap to clone — both fields are
/// already `Arc`-backed under the hood.
#[derive(Clone)]
pub struct TaskSpawner {
    pub tracker: TaskTracker,
    pub shutdown: CancellationToken,
}

impl TaskSpawner {
    /// Build a spawner from the live `AppState`. The single place where
    /// tasks couple to the global state.
    pub fn from_state(state: &AppState) -> Self {
        Self {
            tracker: state.task_tracker.clone(),
            shutdown: state.shutdown_token.clone(),
        }
    }

    /// Spawn a tracked task that runs to completion on its own. The
    /// shutdown token is **not** wired automatically — use this when the
    /// future has its own terminal condition (a single `await`, a channel
    /// close), not for `loop`s.
    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tracker.spawn(fut);
    }

    /// Spawn a tracked task that receives a clone of the shutdown token
    /// and is expected to exit when it fires. The canonical shape for
    /// loops:
    ///
    /// ```ignore
    /// spawner.spawn_cancellable(|shutdown| async move {
    ///     loop {
    ///         tokio::select! {
    ///             () = shutdown.cancelled() => break,
    ///             event = rx.recv() => { /* … */ }
    ///         }
    ///     }
    /// });
    /// ```
    pub fn spawn_cancellable<F, Fut>(&self, f: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        self.tracker.spawn(async move { f(shutdown).await });
    }
}
