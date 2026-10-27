//! The shape a once-per-install sweep runs in: a pass no rescan can reach, gated on a
//! `settings.json` marker rather than inferred. [`OnFailure`] is the one knob, being what the
//! sweeps genuinely differ on.

use crate::services;
use crate::services::settings::LibraryFlags;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::{AppResult, describe};

/// Whether a pass that failed part way still records its marker.
#[derive(Clone, Copy)]
pub enum OnFailure {
    /// Mark anyway. For a pass whose partial result is still correct and which the next scan
    /// repairs, where retrying the same failure every launch buys nothing.
    Mark,
    /// Leave the marker down. For a pass nothing else repairs, where recording one would put
    /// what it was after out of reach for the life of the install.
    Retry,
}

#[derive(Clone, Copy)]
pub struct Sweep {
    /// Names the pass in the two lines below. Prose rather than the flag name, these landing in
    /// the log tail a bug report carries.
    pub label: &'static str,
    /// The `settings.json` flag [`mark`](Self::mark) raises, for `persist_blocking`'s own line.
    pub marker: &'static str,
    pub done: fn(&LibraryFlags) -> bool,
    pub mark: fn(&mut LibraryFlags),
    pub on_failure: OnFailure,
}

impl Sweep {
    /// A marker over a failure puts what the pass was after out of reach for the install's life;
    /// one withheld over a success spends the pass again every launch.
    fn records_marker(self, passed: bool) -> bool {
        passed || matches!(self.on_failure, OnFailure::Mark)
    }
}

/// Runs `pass` unless this install already had one. The marker goes down through
/// `mutate_settings`: the pass takes minutes, and writing back the snapshot read before it would
/// revert every setting changed meanwhile.
pub fn spawn<F, Fut>(spawner: &TaskSpawner, state: &AppState, sweep: Sweep, pass: F)
where
    F: FnOnce(AppState) -> Fut + Send + 'static,
    Fut: Future<Output = AppResult<()>> + Send,
{
    let state = state.clone();
    spawner.spawn(async move {
        match services::settings::read_settings(&state.paths) {
            Ok(settings) if (sweep.done)(&settings.library) => return,
            Ok(_) => {}
            Err(e) => {
                log::warn!("{} skipped: {}", sweep.label, describe(&e));
                return;
            }
        }

        let passed = match pass(state.clone()).await {
            Ok(()) => true,
            Err(e) => {
                log::warn!("{} failed: {}", sweep.label, describe(&e));
                false
            }
        };
        if !sweep.records_marker(passed) {
            return;
        }

        state.persist_blocking(sweep.marker, move |paths| {
            services::settings::mutate_settings(paths, |settings| {
                (sweep.mark)(&mut settings.library);
            })
        });
    });
}

#[cfg(test)]
#[path = "tests/one_shot_tests.rs"]
mod tests;
