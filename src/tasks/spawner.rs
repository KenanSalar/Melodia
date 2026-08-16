//! [`TaskSpawner`] — the single primitive every background task in this module uses
//! to register itself on the shared shutdown lifecycle.
//!
//! [`TaskSpawner::spawn`] is for fire-and-forget work with its own terminal condition
//! (a `sleep` deadline, a channel close); [`TaskSpawner::spawn_cancellable`] for a
//! `loop` / `select!` that has to exit on shutdown. Bundling the pair is what stops a
//! new task reaching into `AppState` by hand and forgetting the cancel arm, which pins
//! runtime shutdown indefinitely.

use std::future::Future;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::state::AppState;

/// Bundled task lifecycle primitives. Cheap to clone — both fields are already
/// `Arc`-backed underneath.
#[derive(Clone)]
pub struct TaskSpawner {
    pub tracker: TaskTracker,
    pub shutdown: CancellationToken,
}

impl TaskSpawner {
    /// Build a spawner from the live `AppState` — the single place where tasks couple
    /// to the global state.
    pub fn from_state(state: &AppState) -> Self {
        Self {
            tracker: state.task_tracker.clone(),
            shutdown: state.shutdown_token.clone(),
        }
    }

    /// Spawn a tracked task that runs to completion on its own. The shutdown token is
    /// **not** wired automatically, so this is for a future with its own terminal
    /// condition — a single `await`, a channel close — never for a `loop`.
    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tracker.spawn(fut);
    }

    /// Spawn a tracked task handed a clone of the shutdown token, which it is expected
    /// to `select!` on and exit when it fires. The canonical shape for loops.
    pub fn spawn_cancellable<F, Fut>(&self, f: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        self.tracker.spawn(async move { f(shutdown).await });
    }
}
