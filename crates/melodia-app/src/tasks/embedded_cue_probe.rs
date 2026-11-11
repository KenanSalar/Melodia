//! Splits the FLACs a library read before the scan looked inside them for a cue sheet, once.
//!
//! `scanner::file_is_current` skips an unchanged file, so a whole-disc FLAC carrying its sheet would
//! stay one long track however often it is rescanned. **Only the files carrying one are read
//! again**: the probe reads each whole-file FLAC's block headers and comment keys, a few kilobytes
//! and no hash, and blanks those rows' stamps alone, where `tag_backfill`'s pass would send every
//! FLAC in the library back through a hash and a full read.

use std::path::Path;

use crate::library;
use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::{AppError, AppResult, describe};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::media::cue;
use melodia_store::media::ingest::scan_pool::ScanPool;

/// Rows held and probed per page, which keeps a library-sized pass off the RSS budget.
const PAGE_ROWS: i64 = 2_000;

/// [`one_shot::OnFailure::Retry`]: nothing else marks these rows stale, and a pass run again only
/// reads headers.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    one_shot::spawn(
        spawner,
        state,
        one_shot::Sweep {
            label: "Embedded cue sheet probe",
            marker: "embedded_cues_probed",
            done: |flags| flags.embedded_cues_probed,
            mark: |flags| flags.embedded_cues_probed = true,
            on_failure: one_shot::OnFailure::Retry,
        },
        |state| async move { probe(&state).await },
    );
}

async fn probe(state: &AppState) -> AppResult<()> {
    let marked = mark_carriers(&state.db).await?;
    if marked == 0 {
        return Ok(());
    }
    log::info!("Embedded cue sheet probe: {marked} FLAC(s) queued for a re-read");

    // A reconcile already in flight takes this request; the marks are durable, so a folder it has
    // already passed picks them up at its next scan.
    library::scan::reconcile_watched_folders(state);
    Ok(())
}

/// Blanks the stamp of every whole-file FLAC carrying a sheet, a page at a time, answering how
/// many.
async fn mark_carriers(db: &DbPool) -> AppResult<u64> {
    let mut after_id = 0;
    let mut marked = 0;
    loop {
        let page = queries::track::get_whole_flac_paths_after(db, after_id, PAGE_ROWS).await?;
        let Some(last_id) = page.last().map(|(id, _)| *id) else {
            break;
        };
        after_id = last_id;

        let carriers = tokio::task::spawn_blocking(move || {
            ScanPool::for_files(page.len()).install(|| carriers(&page))
        })
        .await
        .map_err(|e| AppError::scanner("Embedded cue sheet probe panicked", e))?;
        if !carriers.is_empty() {
            marked += queries::track::mark_tracks_stale(db, carriers).await?;
        }
    }
    Ok(marked)
}

/// **Blocking**: the ids of `rows` whose file carries a sheet. A file that can't be read is left to
/// the scan, which reads it once it changes.
fn carriers(rows: &[(i64, String)]) -> Vec<i64> {
    use rayon::prelude::*;

    rows.par_iter()
        .filter(|(_, path)| {
            cue::carries_sheet(Path::new(path)).unwrap_or_else(|e| {
                log::debug!("Embedded cue sheet probe: {}", describe(&e));
                false
            })
        })
        .map(|(id, _)| *id)
        .collect()
}
