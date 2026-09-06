//! The shape a once-per-install sweep runs in.
//!
//! Two passes fix something no rescan can reach — `scanner::track_is_current` skips a file whose
//! size and mtime haven't moved, so anything the scan learned to read *after* a library was
//! indexed stays unread however many times it is rescanned. Both are gated on a marker in
//! `settings.json` rather than inferred, both are slow passes over files, and neither is an
//! `SQLx` migration for the same reason: a migration failure is fatal at boot, and this must
//! never be able to stop the app opening.
//!
//! What the two genuinely differ on is [`OnFailure`], and that is the only knob here.

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

/// What distinguishes one sweep from the other: what it is called, which flag records it, and
/// what a failure costs.
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
    /// Whether a pass that ended `passed` records its marker.
    ///
    /// The only asymmetry in this module, and the only one that costs anything: a marker recorded
    /// over a failure puts whatever the pass was after out of reach for the life of the install,
    /// and one withheld over a success spends the pass again on every launch.
    fn records_marker(self, passed: bool) -> bool {
        passed || matches!(self.on_failure, OnFailure::Mark)
    }
}

/// Run `pass` unless this install has already had one, then record that it has.
///
/// The marker goes down through `mutate_settings` rather than a write-back of the snapshot read
/// at the top: the pass between the two is minutes long on a real library, and a full-file write
/// of a read that old reverts every setting changed while it ran.
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

        state.persist_blocking(sweep.marker, move |state| {
            services::settings::mutate_settings(&state.paths, |settings| {
                (sweep.mark)(&mut settings.library);
            })
        });
    });
}

#[cfg(test)]
#[path = "tests/one_shot_tests.rs"]
mod tests;
