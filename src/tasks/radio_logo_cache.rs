//! Holds the browsed-logo cache to its bounds, and retires the files that fall out of it.
//!
//! **Two halves that have to run in this order and nowhere else.** `library::radio` decides which
//! answers are still worth keeping and drops the rest, which is what stops referencing their files;
//! [`super::artwork_sweep`] then retires whatever no column names. Reversed, the sweep would see
//! every dropped row's file still referenced and the store would never shrink.
//!
//! **Run when Radio is done with rather than after a scan.** The store only grows while the page
//! is open, so a leave is exactly when it stops — where the artwork sweep's own trigger is a
//! library scan, which a user who browses radio and never touches their music folders may not run
//! for weeks.

use crate::error::AppError;
use crate::library;
use crate::services;
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Prune the answer table and sweep the store behind it, in the background.
///
/// Detached rather than awaited: the caller is a section leave, and one query plus a directory
/// listing is maintenance nothing on screen is waiting for. Tracked, so a shutdown landing
/// mid-sweep waits for the unlinks rather than tearing the runtime down under them.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let state = state.clone();
    spawner.spawn(async move {
        if let Err(e) = run(&state).await {
            log::warn!("Radio logo cache pass failed: {}", services::describe(&e));
        }
    });
}

async fn run(state: &AppState) -> Result<(), AppError> {
    let dropped = library::radio::prune_logo_answers(state).await?;
    if dropped > 0 {
        log::debug!("radio: {dropped} cached logo answer(s) fell outside the cache bounds");
    }
    super::artwork_sweep::run_radio_logos(&state.db, &state.paths).await
}
