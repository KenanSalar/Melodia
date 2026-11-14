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
use melodia_core::utils::reach;
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
    let tally = mark_carriers(&state.db).await?;
    if tally.marked > 0 {
        log::info!("Embedded cue sheet probe: {} FLAC(s) queued for a re-read", tally.marked);
        // A reconcile already in flight takes this request; the marks are durable, so a folder it
        // has already passed picks them up at its next scan.
        library::scan::reconcile_watched_folders(state);
    }
    if tally.out_of_reach > 0 {
        return Err(AppError::scanner_msg(format!(
            "{} FLAC(s) were out of reach, probed again next launch",
            tally.out_of_reach
        )));
    }
    Ok(())
}

#[derive(Default)]
struct Tally {
    marked: u64,
    out_of_reach: usize,
}

/// Blanks the stamp of every whole-file FLAC carrying a sheet, a page at a time.
async fn mark_carriers(db: &DbPool) -> AppResult<Tally> {
    let mut after_id = 0;
    let mut tally = Tally::default();
    loop {
        let page = queries::track::get_whole_flac_paths_after(db, after_id, PAGE_ROWS).await?;
        let Some(last_id) = page.last().map(|(id, _)| *id) else {
            break;
        };
        after_id = last_id;

        let probed = tokio::task::spawn_blocking(move || {
            ScanPool::for_files(page.len()).install(|| probe_page(&page))
        })
        .await
        .map_err(|e| AppError::scanner("Embedded cue sheet probe panicked", e))?;
        tally.out_of_reach += probed.out_of_reach;
        if !probed.carriers.is_empty() {
            tally.marked += queries::track::mark_tracks_stale(db, probed.carriers).await?;
        }
    }
    Ok(tally)
}

struct ProbedPage {
    carriers: Vec<i64>,
    out_of_reach: usize,
}

enum Probed {
    Sheet,
    NoSheet,
    OutOfReach,
}

/// **Blocking**: which files of `rows` carry a sheet, and how many couldn't be reached.
fn probe_page(rows: &[(i64, String)]) -> ProbedPage {
    use rayon::prelude::*;

    let probed: Vec<(i64, Probed)> =
        rows.par_iter().map(|(id, path)| (*id, probe_file(Path::new(path)))).collect();
    let mut page = ProbedPage { carriers: Vec::new(), out_of_reach: 0 };
    for (id, probed) in probed {
        match probed {
            Probed::Sheet => page.carriers.push(id),
            Probed::NoSheet => {}
            Probed::OutOfReach => page.out_of_reach += 1,
        }
    }
    page
}

/// A file that is there but won't read holds nothing the scan could split either, while one out of
/// reach may carry a sheet once it reads.
fn probe_file(path: &Path) -> Probed {
    match cue::carries_sheet(path) {
        Ok(true) => Probed::Sheet,
        Ok(false) => Probed::NoSheet,
        Err(e) => {
            log::debug!("Embedded cue sheet probe: {}", describe(&e));
            if reach::is_out_of_reach(path, &e) { Probed::OutOfReach } else { Probed::NoSheet }
        }
    }
}
