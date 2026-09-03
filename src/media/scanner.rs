use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;
use walkdir::WalkDir;

use super::is_audio_extension;
use crate::entities::scan::{ExistingTrackSummary, ScannedFile};
use crate::media::metadata::extract_or_filename_row;

#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub folder_id: i64,
    pub scanned: u32,
    pub total: u32,
    pub current_file: String,
}

pub fn collect_media_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::with_capacity(256);

    for entry in
        WalkDir::new(dir).follow_links(false).into_iter().filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        if is_audio_extension(ext) {
            files.push(path.to_path_buf());
        }
    }

    files
}

pub fn scan_files_parallel(
    files: &[PathBuf],
    artwork_dir: &Path,
    cover_cache: &crate::media::artwork::CoverCache,
    progress_callback: &(dyn Fn(u32, &str) + Send + Sync),
) -> Vec<ScannedFile> {
    let total = files.len();
    let scanned = std::sync::atomic::AtomicU32::new(0);

    files
        .par_iter()
        .filter_map(|path| {
            let current = scanned.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;

            // Report progress every 10 files
            if current.is_multiple_of(10) || current as usize == total {
                let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                progress_callback(current, file_name);
            }

            // Every file reaching this point was selected by the caller's
            // incremental filter (`track_is_current`) as new or changed, so
            // a full extract — embedded artwork included — is always
            // warranted. Unchanged files never get here.
            match extract_or_filename_row(path, artwork_dir, cover_cache, false) {
                Ok(metadata) => Some(ScannedFile {
                    path: path.clone(),
                    metadata,
                }),
                // Only an unreadable file gets this far now; unparseable tags come back
                // as a filename-derived row rather than a `None`.
                Err(e) => {
                    log::warn!("Skipping {}: {}", path.display(), crate::error::describe(&e));
                    None
                }
            }
        })
        .collect()
}

/// True when the on-disk file matches its existing DB row and needs no
/// re-scan: a track already exists at this path and the file's size **and**
/// mtime are both unchanged. Size + mtime unchanged is a heuristic — not a
/// byte-identity guarantee — but it reliably catches tag and artwork edits,
/// since any normal write bumps the mtime. Callers use this as an
/// incremental-scan filter: a `true` result means Lofty can be skipped
/// entirely for the file.
///
/// A `false` result — no row, or a changed size/mtime — means the file must
/// be (re)parsed in full so its metadata and cover are brought up to date.
pub fn track_is_current<S: std::hash::BuildHasher>(
    path: &Path,
    existing: &HashMap<String, ExistingTrackSummary, S>,
) -> bool {
    let path_str = path.to_string_lossy();
    let Some(row) = existing.get(path_str.as_ref()) else {
        return false;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let on_disk_size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    if row.file_size != Some(on_disk_size) {
        return false;
    }
    // Derive the mtime string from the `meta` already in hand — no second
    // `stat`. Goes through the shared formatter so it can't drift from the
    // format `extract_date_modified` stored in `date_modified`.
    let on_disk_mtime = crate::media::metadata::date_modified_from_metadata(&meta);
    on_disk_mtime.is_some() && on_disk_mtime.as_deref() == row.date_modified.as_deref()
}

#[cfg(test)]
#[path = "tests/scanner_tests.rs"]
mod tests;
