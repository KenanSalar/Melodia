//! Splits the FLACs a library read before the scan looked inside them for a cue sheet, once.
//!
//! `scanner::file_is_current` skips an unchanged file, so a whole-disc FLAC carrying its sheet would
//! stay one long track however often it is rescanned. **Only the files carrying one are read
//! again**: the probe reads each whole-file FLAC's block headers and comment keys, a few kilobytes
//! and no hash, and blanks those rows' stamps alone, where `tag_backfill`'s pass would send every
//! FLAC in the library back through a hash and a full read.

use lofty::file::FileType;

use crate::library;
use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::AppResult;
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::media::cue;

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
    one_shot::require_all_reached(tally.out_of_reach, "FLAC(s)")
}

#[derive(Default)]
struct Tally {
    marked: u64,
    out_of_reach: usize,
}

/// Blanks the stamp of every whole-file FLAC carrying a sheet.
async fn mark_carriers(db: &DbPool) -> AppResult<Tally> {
    let mut tally = Tally::default();
    let mut pages = one_shot::Pages::default();
    while let Some(page) = pages
        .next_page(|after_id| {
            queries::track::get_library_file_paths_after(
                db,
                &[FileType::Flac],
                after_id,
                one_shot::PAGE_ROWS,
            )
        })
        .await?
    {
        let probed =
            one_shot::read_page(page, |rows| one_shot::read_files(rows, cue::carries_sheet))
                .await?;
        tally.out_of_reach += probed.out_of_reach;
        let carriers: Vec<i64> =
            probed.read.into_iter().filter_map(|(id, sheet)| sheet.then_some(id)).collect();
        if !carriers.is_empty() {
            tally.marked += queries::track::mark_tracks_stale(db, carriers).await?;
        }
    }
    Ok(tally)
}
