//! Brings an existing artwork store inside the size bounds the writers now enforce, once.
//!
//! [`crate::media::artwork::store_image`] only reaches files scanned after it shipped:
//! `scanner::track_is_current` skips an unchanged track, so its cover is never re-derived and an
//! install that scanned its library last year keeps whatever its tags happened to carry — up to
//! the 8192 px `MAX_SOURCE_DIM` allows, decoded whole by every tier that draws it.
//!
//! **Not an `SQLx` migration.** It is a slow pass over a rebuildable cache, and a migration failure
//! is fatal at boot — this must never be able to stop the app opening.
//!
//! Nothing is deleted here. A re-encoded cover lands under a new content hash, the rows are
//! re-pointed, and the file they used to name is left unreferenced for the sweep to retire.

use std::path::{Path, PathBuf};

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::{AppError, AppResult};
use crate::media::artwork;
use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};

/// What the pass did, for one log line at the end.
#[derive(Default)]
struct Outcome {
    /// Files that came back smaller and had their rows re-pointed.
    shrunk: u32,
    /// Bytes the store lost. The old files go on the sweep below.
    saved: u64,
    /// Files the pass left where they were — inside both bounds, spared by the never-inflate
    /// rule, or not readable at all.
    left_alone: u32,
}

/// Run the pass unless this install has already had one.
///
/// [`OnFailure::Mark`]: a store that half-normalized is still correct — every row points at a file
/// that exists — and retrying the same failure on every launch buys nothing a re-scan wouldn't.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    one_shot::spawn(
        spawner,
        state,
        one_shot::Sweep {
            label: "Artwork renormalize",
            marker: "artwork_store_normalized",
            done: |flags| flags.artwork_store_normalized,
            mark: |flags| flags.artwork_store_normalized = true,
            on_failure: one_shot::OnFailure::Mark,
        },
        |state| async move { renormalize(&state.db, &state.paths).await },
    );
}

async fn renormalize(db: &DbPool, paths: &Paths) -> AppResult<()> {
    let stored = queries::artwork::referenced_paths(db).await?;
    if stored.is_empty() {
        return Ok(());
    }

    // Only referenced files are worth touching: an orphan is the sweep's to delete, not ours to
    // re-encode. `store_image` re-checks the bounds itself, so handing it everything and keeping
    // what moves is the same answer as pre-filtering, through the one funnel rather than a second
    // copy of its rules.
    let owned: Vec<PathBuf> = stored.into_iter().map(PathBuf::from).collect();
    let steps = tokio::task::spawn_blocking(move || restore_each(&owned))
        .await
        .map_err(AppError::io_source)?;

    let mut outcome = Outcome::default();
    let mut moves = Vec::new();
    for step in steps {
        match step {
            Step::Unchanged => outcome.left_alone += 1,
            Step::Shrunk { from, to, saved } => {
                moves.push((from, to));
                outcome.shrunk += 1;
                outcome.saved += saved;
            }
        }
    }

    if moves.is_empty() {
        return Ok(());
    }

    // One transaction for the whole pass, so no window exists where a row names a file the sweep
    // below is about to retire.
    queries::artwork::repoint_all(db, &moves).await?;
    log::info!(
        "Normalized {} stored cover(s), {} KiB smaller ({} left alone)",
        outcome.shrunk,
        outcome.saved / 1024,
        outcome.left_alone
    );

    // Awaited here rather than left to the next scan: every original this pass replaced is an
    // orphan *created by the re-points above*, so a sweep ordered before them cannot see one —
    // which is exactly what left them on disk for a whole extra launch.
    super::artwork_sweep::run(db, paths).await
}

/// One file's verdict.
enum Step {
    /// Inside both bounds already, unreadable, or the re-encode would have grown it.
    Unchanged,
    Shrunk {
        from: String,
        to: String,
        saved: u64,
    },
}

/// **Blocking** — one read and, for anything over the bounds, one decode plus one encode.
///
/// Serial where the sibling one-shot (`retroactive_hash`) fans out over Rayon, because the work is
/// not the same shape: that one streams a hash, this one holds a whole decoded source, and
/// `MAX_SOURCE_DIM` puts no useful ceiling on one of those times the worker count.
fn restore_each(paths: &[PathBuf]) -> Vec<Step> {
    paths.iter().map(|path| restore_one(path)).collect()
}

fn restore_one(path: &Path) -> Step {
    let Some(dir) = path.parent() else {
        return Step::Unchanged;
    };
    let Ok(bytes) = std::fs::read(path) else {
        // A row pointing at a file that isn't there is the sweep's problem, not this pass's.
        return Step::Unchanged;
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("jpg");

    let Some(stored) = artwork::store_image(&bytes, ext, dir) else {
        return Step::Unchanged;
    };
    let from = path.to_string_lossy().into_owned();
    if stored == from {
        return Step::Unchanged;
    }

    let source_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let saved =
        std::fs::metadata(&stored).ok().map_or(0, |meta| source_len.saturating_sub(meta.len()));
    Step::Shrunk {
        from,
        to: stored,
        saved,
    }
}
