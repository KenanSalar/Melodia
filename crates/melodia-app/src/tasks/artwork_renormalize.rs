//! Brings an existing artwork store inside the size bounds the writers enforce, once:
//! [`melodia_artwork::media::image::artwork::store_image`] bounds only what a scan writes, and
//! `scanner::track_is_current` never re-derives an unchanged track's cover.
//!
//! **Not an `SQLx` migration.** It is a slow pass over a rebuildable cache, and a migration failure
//! is fatal at boot. Nothing is deleted here: a re-encoded cover lands under a new hash, and the
//! file it replaces is left for the sweep.

use std::path::{Path, PathBuf};

use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};
use melodia_artwork::media::image::artwork;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult};
use melodia_store::database::DbPool;
use melodia_store::database::queries;

/// What the pass did, for one log line at the end.
#[derive(Default)]
struct Outcome {
    /// Files that came back smaller and had their rows re-pointed.
    shrunk: u32,
    /// Bytes the store lost.
    saved: u64,
    /// Inside both bounds, spared by the never-inflate rule, or unreadable.
    left_alone: u32,
}

/// [`one_shot::OnFailure::Mark`]: a half-normalized store is still correct, every row naming a
/// file that exists, and a retry each launch buys nothing a rescan wouldn't.
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

    // Referenced files only, an orphan being the sweep's. `store_image` re-checks the bounds, so
    // handing it everything gives pre-filtering's answer without a second copy of its rules.
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

    // One transaction, so no row ever names a file the sweep below may retire.
    queries::artwork::repoint_all(db, &moves).await?;
    log::info!(
        "Normalized {} stored cover(s), {} KiB smaller ({} left alone)",
        outcome.shrunk,
        outcome.saved / 1024,
        outcome.left_alone
    );

    // Awaited rather than left to the next scan: the originals become orphans only through the
    // re-points above, so no earlier sweep could see them.
    super::artwork_sweep::run(db, paths).await.map(drop)
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

/// **Blocking**, and serial where `retroactive_hash` fans out: each step holds a whole decoded
/// source, and `MAX_SOURCE_DIM` sets no useful ceiling on one of those per worker.
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
    Step::Shrunk { from, to: stored, saved }
}

#[cfg(test)]
#[path = "tests/artwork_renormalize_tests.rs"]
mod tests;
