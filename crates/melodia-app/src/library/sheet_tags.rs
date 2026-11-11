//! Edit Tags on a cue sheet's tracks: what the sheet holds is written into it, and its tracks are
//! read back as a scan reads an edited sheet, so each keeps its id, play count and playlist places.
//! Nothing else of the image is touched, its tags stating the whole disc: a sheet inside the image
//! is its `CUESHEET` comment alone.
//!
//! The read lands through the import's commit, which files a sheet outside every library folder
//! under a folder of its own and links its tracks to nothing; the watcher's would skip that sheet.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::import::{Reading, ingest, parent_dirs};
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::entities::cue::{SheetFile, SheetTrack};
use melodia_core::error::{AppError, describe};
use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::database::queries::scan;
use melodia_store::database::{DbPool, queries};
use melodia_store::media::cue::{self, SheetEdit, SheetRows};

/// What the sheet pass did with each sheet behind the edited tracks.
#[derive(Default)]
pub(super) struct SheetWrites {
    /// The tracks whose rows now state the edit.
    pub(super) updated: Vec<i64>,
    /// `(sheet, error)` for each sheet whose write or read-back failed.
    pub(super) failures: Vec<(String, String)>,
}

/// An edit and the sheets behind its tracks, read before anything is written.
pub(super) struct SheetPass {
    edit: SheetEdit,
    sheets: BTreeMap<SheetFile, SelectedTracks>,
}

impl SheetPass {
    /// The sheets behind the cue sheet tracks among `ids`, with the artist each of their tracks
    /// shows. Empty where `edit` holds nothing a sheet does.
    pub(super) async fn read(db: &DbPool, ids: &[i64], edit: SheetEdit) -> Result<Self, AppError> {
        if edit.is_empty() {
            return Ok(Self { edit, sheets: BTreeMap::new() });
        }
        let mut sheets = by_sheet(queries::track::get_sheet_tracks_by_ids(db, ids).await?);
        if !sheets.is_empty() {
            let keys = sheets.keys().cloned().collect();
            let mut artists = queries::track::get_sheet_artists(db, keys).await?;
            for (sheet, selected) in &mut sheets {
                selected.rows.artists = artists.remove(sheet).unwrap_or_default();
            }
        }
        Ok(Self { edit, sheets })
    }

    /// Writes the edit into each sheet, then reads every sheet it changed back in.
    pub(super) async fn write(
        self,
        db: &DbPool,
        artwork_dir: &Path,
        cover_cache: &CoverCache,
        self_writes: &Arc<SelfWrites>,
    ) -> Result<SheetWrites, AppError> {
        let Self { edit, sheets } = self;
        if sheets.is_empty() {
            return Ok(SheetWrites::default());
        }
        let marks = Arc::clone(self_writes);
        let written: Vec<SheetWrite> = tokio::task::spawn_blocking(move || {
            let write = |(sheet, selected): (SheetFile, SelectedTracks)| {
                let outcome = write_sheet(&sheet, &selected.rows, &edit, &marks);
                SheetWrite { sheet, ids: selected.ids, outcome }
            };
            sheets.into_iter().map(write).collect()
        })
        .await
        .map_err(|e| AppError::metadata("cue sheet write task panicked", e))?;

        let mut writes = SheetWrites::default();
        let mut changed: Vec<(SheetFile, Vec<i64>)> = Vec::new();
        for SheetWrite { sheet, ids, outcome } in written {
            match outcome {
                Ok(true) => changed.push((sheet, ids)),
                Ok(false) => writes.updated.extend(ids),
                Err(e) => {
                    let reason = describe(&e);
                    log::warn!("tag write failed for {}: {reason}", sheet.path());
                    writes.failures.push((sheet.path().to_owned(), reason));
                }
            }
        }
        if changed.is_empty() {
            return Ok(writes);
        }

        let sheets: Vec<SheetFile> = changed.iter().map(|(sheet, _)| sheet.clone()).collect();
        match read_back(db, artwork_dir, cover_cache, &sheets).await {
            Ok(()) => writes.updated.extend(changed.into_iter().flat_map(|(_, ids)| ids)),
            Err(e) => {
                // Unmarked so the watcher reads them, rather than leaving the rows stale.
                let paths: Vec<PathBuf> = sheets.iter().map(|sheet| sheet.path().into()).collect();
                self_writes.unmark(&paths);
                let reason = describe(&e);
                log::warn!("cue sheet read-back failed: {reason}");
                let failed =
                    changed.into_iter().map(|(sheet, _)| (sheet.path().to_owned(), reason.clone()));
                writes.failures.extend(failed);
            }
        }
        Ok(writes)
    }
}

/// The tracks an edit selected out of one sheet.
#[derive(Default)]
struct SelectedTracks {
    ids: Vec<i64>,
    rows: SheetRows,
}

fn by_sheet(tracks: Vec<SheetTrack>) -> BTreeMap<SheetFile, SelectedTracks> {
    let mut sheets: BTreeMap<SheetFile, SelectedTracks> = BTreeMap::new();
    for SheetTrack { id, sheet, track } in tracks {
        let selected = sheets.entry(sheet).or_default();
        selected.ids.push(id);
        selected.rows.edited.push(track);
    }
    sheets
}

/// One sheet's write, `Ok(false)` where it already stated the edit.
struct SheetWrite {
    sheet: SheetFile,
    ids: Vec<i64>,
    outcome: Result<bool, AppError>,
}

/// Writes `edit` into `sheet` for its tracks `rows`, answering whether the file changed. Each file
/// is marked before its write and by the path on record, as a file's tag write marks it. Blocking.
fn write_sheet(
    sheet: &SheetFile,
    rows: &SheetRows,
    edit: &SheetEdit,
    self_writes: &SelfWrites,
) -> Result<bool, AppError> {
    let path = Path::new(sheet.path());
    match sheet {
        SheetFile::Cue(_) => {
            let Some(bytes) = cue::rewritten_sheet(path, edit, rows)? else {
                return Ok(false);
            };
            self_writes.mark(path);
            cue::replace_sheet(path, &bytes)?;
        }
        SheetFile::Embedded(_) => {
            let Some(image) = cue::rewritten_embedded(path, edit, rows)? else {
                return Ok(false);
            };
            self_writes.mark(path);
            image.save(path)?;
        }
    }
    Ok(true)
}

/// Reads `written` back in with the images they hold, and claims no other: the settle reads every
/// sheet beside a written `.cue`, and one beside a sheet opened from outside the library was never
/// opened. An image whose own sheet was written is read as an opened file is.
async fn read_back(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    written: &[SheetFile],
) -> Result<(), AppError> {
    let mut sheets: Vec<PathBuf> = Vec::new();
    let mut images: Vec<PathBuf> = Vec::new();
    for sheet in written {
        match sheet {
            SheetFile::Cue(path) => sheets.push(path.into()),
            SheetFile::Embedded(path) => images.push(path.into()),
        }
    }
    let around = scan::sheets_around(db, parent_dirs(&[], &sheets), Vec::new()).await?;
    let held: HashSet<PathBuf> = around
        .claims
        .iter()
        .filter(|(_, sheet)| sheets.iter().any(|written| written == Path::new(sheet)))
        .map(|(image, _)| PathBuf::from(image))
        .collect();
    let owns = move |image: &Path| held.contains(image);
    let reading = Reading { new_audio: images, sheets, around, owns };
    ingest(db, reading, artwork_dir, cover_cache).await?;
    Ok(())
}
