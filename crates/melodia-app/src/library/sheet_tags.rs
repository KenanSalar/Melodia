//! Edit Tags on a cue sheet's tracks: what the sheet holds is written into it, and its tracks are
//! read back as a scan reads an edited sheet, so each keeps its id, play count and playlist places.
//! Nothing reaches the image, whose tags state the whole disc.
//!
//! The read lands through the import's commit, which files a sheet outside every library folder
//! under a folder of its own and links its tracks to nothing; the watcher's would skip that sheet.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::import::{Reading, read_in};
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::entities::cue::{CueTrack, SheetTrack};
use melodia_core::error::{AppError, describe};
use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::database::queries::scan;
use melodia_store::database::{DbPool, queries};
use melodia_store::media::cue::{self, SheetEdit};

/// What the sheet pass did with each sheet behind the edited tracks.
#[derive(Default)]
pub(super) struct SheetWrites {
    /// The tracks whose rows now state the edit.
    pub(super) updated: Vec<i64>,
    /// `(sheet, error)` for each sheet whose write or read-back failed.
    pub(super) failures: Vec<(String, String)>,
}

/// Writes `edit` into the sheet behind each of `tracks`, then reads every sheet it changed back in.
pub(super) async fn write_sheets(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &Arc<SelfWrites>,
    tracks: Vec<SheetTrack>,
    edit: SheetEdit,
) -> Result<SheetWrites, AppError> {
    if tracks.is_empty() {
        return Ok(SheetWrites::default());
    }

    let sheets = by_sheet(tracks);
    let marks = Arc::clone(self_writes);
    let written: Vec<SheetWrite> = tokio::task::spawn_blocking(move || {
        let write = |(sheet, tracks): (String, SelectedTracks)| {
            let outcome = write_sheet(Path::new(&sheet), &tracks.numbers, &edit, &marks);
            SheetWrite { sheet, ids: tracks.ids, outcome }
        };
        sheets.into_iter().map(write).collect()
    })
    .await
    .map_err(|e| AppError::metadata("cue sheet write task panicked", e))?;

    let mut writes = SheetWrites::default();
    let mut changed: Vec<(String, Vec<i64>)> = Vec::new();
    for SheetWrite { sheet, ids, outcome } in written {
        match outcome {
            Ok(true) => changed.push((sheet, ids)),
            Ok(false) => writes.updated.extend(ids),
            Err(e) => {
                let reason = describe(&e);
                log::warn!("tag write failed for {sheet}: {reason}");
                writes.failures.push((sheet, reason));
            }
        }
    }
    if changed.is_empty() {
        return Ok(writes);
    }

    let paths: Vec<PathBuf> = changed.iter().map(|(sheet, _)| PathBuf::from(sheet)).collect();
    match read_back(db, artwork_dir, cover_cache, paths.clone()).await {
        Ok(()) => writes.updated.extend(changed.into_iter().flat_map(|(_, ids)| ids)),
        Err(e) => {
            // Unmarked so the watcher reads them, rather than leaving the rows stale.
            self_writes.unmark(&paths);
            let reason = describe(&e);
            log::warn!("cue sheet read-back failed: {reason}");
            writes.failures.extend(changed.into_iter().map(|(sheet, _)| (sheet, reason.clone())));
        }
    }
    Ok(writes)
}

/// The tracks an edit selected out of one sheet.
#[derive(Default)]
struct SelectedTracks {
    ids: Vec<i64>,
    numbers: Vec<CueTrack>,
}

fn by_sheet(tracks: Vec<SheetTrack>) -> BTreeMap<String, SelectedTracks> {
    let mut sheets: BTreeMap<String, SelectedTracks> = BTreeMap::new();
    for SheetTrack { id, sheet, track } in tracks {
        let selected = sheets.entry(sheet).or_default();
        selected.ids.push(id);
        selected.numbers.push(track);
    }
    sheets
}

/// One sheet's write, `Ok(false)` where it already stated the edit.
struct SheetWrite {
    sheet: String,
    ids: Vec<i64>,
    outcome: Result<bool, AppError>,
}

/// Writes `edit` into `sheet` for its tracks numbered `numbers`, answering whether the file
/// changed. Blocking.
fn write_sheet(
    sheet: &Path,
    numbers: &[CueTrack],
    edit: &SheetEdit,
    self_writes: &SelfWrites,
) -> Result<bool, AppError> {
    let Some(bytes) = cue::rewritten_sheet(sheet, edit, numbers)? else {
        return Ok(false);
    };
    // Before the write and by the path on record, as a file's tag write marks it.
    self_writes.mark(sheet);
    cue::replace_sheet(sheet, &bytes)?;
    Ok(true)
}

/// Reads `sheets` back in with the images they hold, and claims no other: the settle reads every
/// sheet beside them, and one beside a sheet opened from outside the library was never opened.
async fn read_back(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    sheets: Vec<PathBuf>,
) -> Result<(), AppError> {
    let dirs: BTreeSet<String> = sheets
        .iter()
        .filter_map(|sheet| sheet.parent())
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    let around = scan::sheets_around(db, dirs.into_iter().collect(), Vec::new()).await?;
    let held: HashSet<PathBuf> = around
        .claims
        .iter()
        .filter(|(_, sheet)| sheets.iter().any(|written| written == Path::new(sheet)))
        .map(|(image, _)| PathBuf::from(image))
        .collect();
    let owns = move |image: &Path| held.contains(image);
    let reading = Reading { new_audio: Vec::new(), sheets, around, owns };
    let (chunk, pool) = read_in(reading, artwork_dir, cover_cache).await?;
    let scan_timestamp = melodia_core::utils::now_rfc3339();
    queries::ingest::commit_import(db, chunk, scan_timestamp, pool).await?;
    Ok(())
}
