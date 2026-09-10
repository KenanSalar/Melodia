//! Re-reads the tags a library carried in before the ingest learned to look for them, once.
//!
//! `scanner::track_is_current` skips a file whose size and mtime haven't moved, so everything the
//! reader gained — role credits, a second genre, sort names, ISRC, the whole-date fields, the
//! release tags, and the `"; "`-flattened `ARTISTS` an ID3v2.3 file carries — stays unread on an
//! existing library however many times it is rescanned. The migration seeds what the database
//! already knew and no more; the rest is in the files.
//!
//! **The pass is one UPDATE.** Blanking `date_modified` is what makes `track_is_current` answer
//! false, and the scan then does the work through the path that is already batched, chunked and
//! cancellable. A second ingest pipeline beside it would have to keep its own column list in step
//! with `TRACK_INSERT_COLUMNS` — which is the duplication the tree's one hand-built UPDATE ban
//! exists to prevent — and would re-hash every file to fill the columns it isn't there to change.
//!
//! Supersedes the artist-credit import, which read one field through a pass of its own. A rescan
//! reaches that field and every other, and it repairs the one thing the import's own doc recorded
//! it could not: an album filed under a whole credit line ("X feat. Y") is re-resolved under the
//! primary name, and `prune_orphans` retires the row it left.
//!
//! **Not an `SQLx` migration**, for [`super::rating_import`]'s reason: a migration failure is fatal
//! at boot, and nothing here may stop the app opening.

use crate::library;
use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::AppResult;
use melodia_store::database::DbPool;

/// Run the backfill unless this install has already had one.
///
/// [`one_shot::OnFailure::Retry`]: nothing else marks these rows stale, so a marker recorded over
/// a failed UPDATE would put every tag this release learned to read out of reach for the life of
/// the install. The pass is idempotent, so a retry costs one more UPDATE.
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
    let marked = mark_every_track_stale(&state.db).await?;
    if marked == 0 {
        return Ok(());
    }

    log::info!("Tag backfill: {marked} track(s) queued for a re-read of their tags");

    // Whichever reconcile gets there first does the work. This one is a no-op when the boot scan
    // is already in flight — the marks are durable, so that pass or the next launch's picks them
    // up, and the marker records the half that must not be lost.
    library::settings::reconcile_watched_folders(state);
    Ok(())
}

/// Blank every `date_modified`, answering with how many rows moved.
///
/// The column means "the mtime the stored row was parsed from", so NULL is honestly "unknown"
/// rather than a sentinel: `track_is_current` compares it to the file's own and a missing value
/// cannot match one. The scan overwrites it from the same `fs::metadata` that proved the file
/// exists, so nothing is left blank behind the pass.
async fn mark_every_track_stale(db: &DbPool) -> AppResult<u64> {
    let result =
        sqlx::query("UPDATE tracks SET date_modified = NULL WHERE date_modified IS NOT NULL")
            .execute(db.write())
            .await?;
    Ok(result.rows_affected())
}
