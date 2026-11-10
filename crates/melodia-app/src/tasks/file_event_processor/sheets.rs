//! The cue sheet side of a watcher batch: the events a sheet claims or could claim, settled by the
//! scan's planner (`cue::settle_dirs`) over the directories the batch touches and those its claims
//! sit in, before anything is read for the write.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use melodia_core::entities::scan::{ExtractedMetadata, ScannedFile};
use melodia_core::error::{AppError, describe};
use melodia_store::database::DbPool;
use melodia_store::database::queries::scan::{self, SheetsAround, WatchedChange};
use melodia_store::media::cue::{self, CueChunk, CuePlan};
use melodia_store::media::ingest::watcher::{FileEvent, is_sheet_file};

/// A batch with its cue sheets settled.
pub(super) struct SheetBatch {
    /// The events left to the ordinary path.
    pub(super) events: Vec<FileEvent>,
    /// The sheet jobs, whose images are read with the batch's other files.
    pub(super) cue: CueChunk,
}

/// Settles the cue sheets `events` touch. A failed read leaves them to the next scan, and the batch
/// goes on without them.
pub(super) async fn settle(db: &DbPool, events: Vec<FileEvent>) -> SheetBatch {
    let mut around = match read_around(db, &events).await {
        Ok(around) => around,
        Err(e) => return leave_to_next_scan(events, &e),
    };
    follow_renamed_images(&mut around.claims, &events);
    let events = split_sheet_renames(events);
    let names_a_sheet = events.iter().flat_map(event_paths).any(is_sheet_file);
    if around.is_empty() && !names_a_sheet {
        return SheetBatch { events, cue: CueChunk::default() };
    }

    let plan = match plan(&events, around).await {
        Ok(plan) => plan,
        Err(e) => return leave_to_next_scan(events, &e),
    };
    let events = events.into_iter().filter(|event| !is_taken_by(&plan, event)).collect();
    SheetBatch { events, cue: plan.into_chunk() }
}

/// The batch with no sheet work. A sheet's own events are safe applied alone: a removal forgets
/// only a sheet no row names.
fn leave_to_next_scan(events: Vec<FileEvent>, e: &AppError) -> SheetBatch {
    log::warn!("Cue sheets in this batch left to the next scan: {}", describe(e));
    SheetBatch { events, cue: CueChunk::default() }
}

/// The batch's sheet change, each image its jobs read moved out of `metadata`, or `None` where it
/// owes no job.
pub(super) fn change(
    cue: CueChunk,
    metadata: &mut HashMap<PathBuf, ExtractedMetadata>,
) -> Option<WatchedChange> {
    if cue.is_empty() {
        return None;
    }
    let images = cue
        .images()
        .into_iter()
        .filter_map(|path| {
            let metadata = metadata.remove(&path)?;
            Some(ScannedFile { path, metadata })
        })
        .collect();
    Some(WatchedChange::Sheets { cue, images })
}

async fn read_around(db: &DbPool, events: &[FileEvent]) -> Result<SheetsAround, AppError> {
    let mut dirs = BTreeSet::new();
    let mut audio = Vec::new();
    for path in events.iter().flat_map(event_paths) {
        if let Some(dir) = path.parent() {
            dirs.insert(dir.to_string_lossy().into_owned());
        }
        if !is_sheet_file(path) {
            audio.push(path.to_string_lossy().into_owned());
        }
    }
    scan::sheets_around(db, dirs.into_iter().collect(), audio).await
}

/// Hands each renamed image's claim to its new name. The rename re-points every row of the path
/// before the sheet's job runs, so the plan judges the image where its rows will be: still named,
/// they update by number; named no more, the image is given back with its playlist places.
fn follow_renamed_images(claims: &mut HashMap<String, String>, events: &[FileEvent]) {
    for event in events {
        if let FileEvent::Renamed { from, to } = event
            && let Some(sheet) = claims.remove(from.to_string_lossy().as_ref())
        {
            claims.insert(to.to_string_lossy().into_owned(), sheet);
        }
    }
}

/// Splits each rename of a sheet into its removal and its creation, which the plan reads as a scan
/// would: the new name claims the images by number, so every row keeps its id.
fn split_sheet_renames(events: Vec<FileEvent>) -> Vec<FileEvent> {
    let mut split = Vec::with_capacity(events.len());
    for event in events {
        match event {
            FileEvent::Renamed { from, to } if is_sheet_file(&from) || is_sheet_file(&to) => {
                split.push(FileEvent::Removed(from));
                split.push(FileEvent::Created(to));
            }
            event => split.push(event),
        }
    }
    split
}

/// Runs the planner over the directories `events` touch, on the blocking pool.
async fn plan(events: &[FileEvent], around: SheetsAround) -> Result<CuePlan, AppError> {
    let touched = Touched::of(events, &around.claims);
    tokio::task::spawn_blocking(move || touched.settle(&around))
        .await
        .map_err(|e| AppError::scanner("Cue sheet planning task failed", e))
}

/// What the planner reads off a batch's events.
struct Touched {
    /// Every directory an event touches, and those of the sheets and images the library holds
    /// claims for, so a sheet planned again finds the images it already claims.
    dirs: Vec<PathBuf>,
    /// The paths the events leave new content at.
    written: Vec<PathBuf>,
}

impl Touched {
    fn of(events: &[FileEvent], claims: &HashMap<String, String>) -> Self {
        let claimed = claims.iter().flat_map(|(image, sheet)| [Path::new(image), Path::new(sheet)]);
        let paths = events.iter().flat_map(event_paths).chain(claimed);
        let dirs: BTreeSet<&Path> = paths.filter_map(Path::parent).collect();
        Self {
            dirs: dirs.into_iter().map(Path::to_path_buf).collect(),
            written: events.iter().filter_map(written_path).map(Path::to_path_buf).collect(),
        }
    }

    /// Settles the directories' sheets against what `around` holds. Each job is filed under the
    /// library folder its own sheet sits in, so no folder's files are kept out here.
    fn settle(self, around: &SheetsAround) -> CuePlan {
        cue::settle_dirs(&self.dirs, self.written, around, |_| true)
    }
}

/// Whether the sheet jobs own `event` in place of the ordinary path: a sheet written to, which the
/// plan has read, or an image the plan settled, which only its sheet's job writes.
fn is_taken_by(plan: &CuePlan, event: &FileEvent) -> bool {
    match event {
        FileEvent::Created(path) | FileEvent::Modified(path) => {
            is_sheet_file(path) || plan.settles(path)
        }
        _ => false,
    }
}

/// The path an event leaves new content at.
fn written_path(event: &FileEvent) -> Option<&Path> {
    match event {
        FileEvent::Created(path) | FileEvent::Modified(path) => Some(path),
        FileEvent::Renamed { to, .. } => Some(to),
        _ => None,
    }
}

fn event_paths(event: &FileEvent) -> impl Iterator<Item = &Path> {
    let (path, renamed_to) = match event {
        FileEvent::Created(path) | FileEvent::Modified(path) | FileEvent::Removed(path) => {
            (path, None)
        }
        FileEvent::Renamed { from, to } => (from, Some(to)),
        // The caller reconciles on these, or has dropped a gone path holding no tracks.
        FileEvent::RescanNeeded | FileEvent::DirectoryRenamed | FileEvent::DirectoryGone(_) => {
            unreachable!()
        }
    };
    std::iter::once(path.as_path()).chain(renamed_to.map(PathBuf::as_path))
}
