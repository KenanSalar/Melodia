//! Sheets read a few directories at a time rather than a library folder: a watcher batch, and files
//! the user opens. Only those directories are listed, so a saved or opened sheet costs a few
//! listings rather than the folder scan's `stat` of every file.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::plan::{CuePlan, PlanInput, read_cue, settle};
use super::resolve::resolve;
use crate::database::queries::scan::SheetsAround;
use crate::media::ingest::scan_pool::ScanPool;
use crate::media::ingest::scanner::{MediaWalk, WalkIndex, list_media_files};
use crate::media::ingest::watcher::is_sheet_file;
use melodia_core::error::AppError;

/// Settles the sheets `around` was read for against what the library holds there, `written` being
/// the paths left with new content. Only the images `owns` admits are claimed or given back.
pub fn settle_dirs(
    around: &SheetsAround,
    written: Vec<PathBuf>,
    owns: impl Fn(&Path) -> bool + Send + Sync,
) -> CuePlan {
    let MediaWalk { files, sheets, unreadable } = list_media_files(&listed_dirs(around));
    ScanPool::for_files(sheets.len()).install(|| {
        let audio = WalkIndex::new(files, unreadable.clone());
        let sheets = WalkIndex::new(sheets, unreadable);
        let (changed_sheets, changed_audio): (Vec<PathBuf>, Vec<PathBuf>) =
            written.into_iter().partition(|path| is_sheet_file(path));
        let changed_audio: Vec<PathBuf> =
            changed_audio.into_iter().filter(|path| audio.contains(path)).collect();
        let changed_sheets: Vec<PathBuf> =
            changed_sheets.into_iter().filter(|path| sheets.contains(path)).collect();
        // A stamp row outlives a sheet whose job rolled back, so a sheet on record the listing
        // didn't find is gone whether or not anything saw it go.
        let gone_sheets = sheets.unwalked(around.sheets.iter().cloned());
        let input = PlanInput {
            audio: &audio,
            sheets: &sheets,
            changed_audio: &changed_audio,
            changed_sheets: &changed_sheets,
            gone_sheets: &gone_sheets,
            claims: &around.claims,
        };
        settle(&input, owns)
    })
}

/// The directories `around` was asked about, and those of each sheet and image it holds a claim
/// for, so a sheet read again finds the images it claims. Every sheet on record there sits in one,
/// so a sheet the listing missed is gone rather than merely unlisted.
fn listed_dirs(around: &SheetsAround) -> Vec<PathBuf> {
    let claimed = around.claims.iter().flat_map(|(image, sheet)| [image, sheet]);
    let claim_dirs = claimed.filter_map(|path| Path::new(path).parent()).map(Path::to_path_buf);
    let dirs: BTreeSet<PathBuf> = around.dirs.iter().map(PathBuf::from).chain(claim_dirs).collect();
    dirs.into_iter().collect()
}

/// The audio files each of `sheets` names, in its order, which is what opening a sheet opens: the
/// image it splits, or a per-track rip's files. A sheet that fails to read, places no track or
/// names no file beside it answers an error.
pub fn named_files(sheets: &[PathBuf]) -> Vec<Result<Vec<PathBuf>, AppError>> {
    let dirs: HashSet<&Path> = sheets.iter().filter_map(|sheet| sheet.parent()).collect();
    let dirs: Vec<PathBuf> = dirs.into_iter().map(Path::to_path_buf).collect();
    let MediaWalk { files, unreadable, .. } = list_media_files(&dirs);
    let audio = WalkIndex::new(files, unreadable);
    sheets.iter().map(|sheet| files_named_by(sheet, &audio)).collect()
}

fn files_named_by(path: &Path, audio: &WalkIndex) -> Result<Vec<PathBuf>, AppError> {
    let sheet = read_cue(path)?;
    let sheet_dir = path.parent().unwrap_or(Path::new(""));
    let mut files: Vec<PathBuf> = Vec::new();
    for name in &sheet.files {
        if let Some(file) = resolve(name, sheet_dir, audio)
            && !files.contains(&file)
        {
            files.push(file);
        }
    }
    if files.is_empty() {
        return Err(AppError::metadata_msg(format!("{} names no audio file", path.display())));
    }
    Ok(files)
}
