//! Gives back, once, the WAVs and AIFFs whose ID3 chunk a tag write broke, and their rows' tags.
//!
//! Nothing else mends one: a rescan reads the same broken chunks. Each mended file lands through a
//! re-read commit rather than a rescan, which keeps its loudness measurement.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lofty::file::FileType;

use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::{AppError, AppResult, describe};
use melodia_store::database::queries;
use melodia_store::media::ingest::iff_repair::{self, Damage, Verdict};
use melodia_store::media::ingest::metadata::extract_metadata;

/// [`one_shot::OnFailure::Retry`]: nothing else mends these files, and a pass run again reads only
/// headers. No switch, since a tag edit broke a chunk as surely as a star.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    one_shot::spawn(
        spawner,
        state,
        one_shot::Sweep {
            label: "WAV and AIFF tag repair",
            marker: "iff_tags_repaired",
            done: |flags| flags.iff_tags_repaired,
            mark: |flags| flags.iff_tags_repaired = true,
            on_failure: one_shot::OnFailure::Retry,
        },
        |state| async move { repair(&state).await },
    );
}

async fn repair(state: &AppState) -> AppResult<()> {
    let tally = repair_damaged_files(state).await?;
    log::info!(
        "WAV and AIFF tag repair: {} file(s) repaired, {} failed",
        tally.written,
        tally.failed
    );
    if tally.written > 0 {
        state.library_changed.bump();
    }
    one_shot::require_all_reached(tally.out_of_reach, "WAV or AIFF file(s)")
}

async fn repair_damaged_files(state: &AppState) -> AppResult<one_shot::Rewrites> {
    let mut tally = one_shot::Rewrites::default();
    let mut pages = one_shot::Pages::default();
    while let Some(page) = pages
        .next_page(|after_id| {
            queries::track::get_whole_file_paths_after(
                &state.db,
                &[FileType::Wav, FileType::Aiff],
                after_id,
                one_shot::PAGE_ROWS,
            )
        })
        .await?
    {
        let files = one_shot::read_page(page, |rows| one_shot::read_files(rows, damage_of)).await?;
        tally
            .rewrite_each(&state.shutdown_token, files, |id, found| {
                repair_if_damaged(state, id, found)
            })
            .await?;
    }
    Ok(tally)
}

/// `path` with its damage, `None` for a clean file. A walk that breaks some other way is left alone.
fn damage_of(path: &Path) -> AppResult<Option<(String, Damage)>> {
    Ok(match iff_repair::diagnose(path)? {
        Verdict::Clean => None,
        Verdict::Damaged(damage) => path.to_str().map(|path| (path.to_owned(), damage)),
        Verdict::Unrecognised { at } => {
            log::debug!(
                "WAV and AIFF tag repair: {} breaks at byte {at}, matching no tag write",
                path.display()
            );
            None
        }
    })
}

/// Marked as this process's write first, so the watcher leaves the rename to the commit, and
/// unmarked wherever no commit follows, so it reads back whatever did land.
async fn repair_if_damaged(
    state: &AppState,
    id: i64,
    found: Option<(String, Damage)>,
) -> AppResult<one_shot::Rewrite> {
    let Some((path, damage)) = found else {
        return Ok(one_shot::Rewrite::Skipped);
    };
    let marked = PathBuf::from(&path);
    let read_back = {
        let self_writes = Arc::clone(&state.self_writes);
        let artwork_dir = state.paths.artwork_dir.clone();
        let cover_cache = state.cover_cache.clone();
        let marked = marked.clone();
        tokio::task::spawn_blocking(move || {
            self_writes.mark(&marked);
            iff_repair::repair(&marked, damage)?;
            extract_metadata(&marked, &artwork_dir, &cover_cache, false)
        })
        .await
        .map_err(|e| AppError::scanner("A tag repair panicked", e))?
    };
    let meta = match read_back {
        Ok(meta) => meta,
        Err(e) => {
            log::warn!("WAV and AIFF tag repair: {path} failed: {}", describe(&e));
            state.self_writes.unmark(&[marked]);
            return Ok(one_shot::Rewrite::Failed);
        }
    };

    let file = queries::scan::RetaggedFile { id, path, meta };
    if let Err(e) = queries::scan::commit_reread(&state.db, vec![file]).await {
        state.self_writes.unmark(&[marked]);
        return Err(e);
    }
    Ok(one_shot::Rewrite::Written)
}
