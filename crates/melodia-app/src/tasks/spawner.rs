//! [`TaskSpawner`]: bundling the tracker with the shutdown token is what stops a new task reaching
//! into `AppState` by hand and forgetting the cancel arm, which pins runtime shutdown.

use std::future::Future;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::state::AppState;

/// Cheap to clone, both fields being `Arc`-backed.
#[derive(Clone)]
pub struct TaskSpawner {
    pub tracker: TaskTracker,
    pub shutdown: CancellationToken,
}

impl TaskSpawner {
    pub fn from_state(state: &AppState) -> Self {
        Self { tracker: state.task_tracker.clone(), shutdown: state.shutdown_token.clone() }
    }

    /// For a future with its own end, a single `await` or a channel close, and never a `loop`: the
    /// shutdown token isn't wired.
    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tracker.spawn(fut);
    }

    /// Hands the task the shutdown token to `select!` on; the shape for loops.
    pub fn spawn_cancellable<F, Fut>(&self, f: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        self.tracker.spawn(async move { f(shutdown).await });
    }
}
