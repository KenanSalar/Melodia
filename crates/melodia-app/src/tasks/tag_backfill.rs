//! Re-reads the tags a library carried in before the ingest learned to look for them, once:
//! `scanner::track_is_current` skips an unchanged file, so everything the reader gained since stays
//! unread however often it is rescanned, the migrations having seeded only what the database knew.
//!
//! **The pass is one UPDATE**, `queries::track::mark_every_track_stale`: a blank `date_modified`
//! sends every file back through the scan's own batched, cancellable path. A second ingest pipeline
//! would have to keep its column list in step with `TRACK_INSERT_COLUMNS` and would re-hash every
//! file. **Not an `SQLx` migration**, for [`super::artwork_renormalize`]'s reason.

use crate::library;
use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::AppResult;
use melodia_store::database::queries;

/// [`one_shot::OnFailure::Retry`]: nothing else marks these rows stale, and a retry costs one more
/// idempotent UPDATE.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    one_shot::spawn(
        spawner,
        state,
        one_shot::Sweep {
            label: "Tag backfill",
            marker: "tags_backfilled",
            done: |flags| flags.tags_backfilled,
            mark: |flags| flags.tags_backfilled = true,
            on_failure: one_shot::OnFailure::Retry,
        },
        |state| async move { backfill(&state).await },
    );
}

async fn backfill(state: &AppState) -> AppResult<()> {
    let marked = queries::track::mark_every_track_stale(&state.db).await?;
    if marked == 0 {
        return Ok(());
    }

    log::info!("Tag backfill: {marked} track(s) queued for a re-read of their tags");

    // A reconcile already in flight takes this request; the marks are durable, so a folder it has
    // already passed picks them up at its next scan.
    library::scan::reconcile_watched_folders(state);
    Ok(())
}
