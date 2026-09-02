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

use crate::error::AppResult;
use crate::services;
use crate::services::settings::LibraryFlags;
use crate::state::AppState;
use crate::tasks::TaskSpawner;

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

/// What distinguishes one sweep from the other: its marker, and what a failure costs.
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
                log::warn!("{} skipped: {}", sweep.label, services::describe(&e));
                return;
            }
        }

        if let Err(e) = pass(state.clone()).await {
            log::warn!("{} failed: {}", sweep.label, services::describe(&e));
            if matches!(sweep.on_failure, OnFailure::Retry) {
                return;
            }
        }

        state.persist_blocking(sweep.marker, move |state| {
            services::settings::mutate_settings(&state.paths, |settings| {
                (sweep.mark)(&mut settings.library);
            })
        });
    });
}
