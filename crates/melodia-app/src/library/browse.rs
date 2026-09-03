use std::collections::HashMap;
use std::path::PathBuf;

use crate::state::AppState;
use melodia_core::entities::browse::{BrowseFile, BrowseFolder, BrowseResult};
use melodia_core::entities::folder::Folder;
use melodia_core::entities::track::TrackListRow;
use melodia_core::error::AppError;
use melodia_core::utils::audio_ext::is_audio_extension;
use melodia_store::database::queries;

/// Result of the blocking filesystem scan, returned from `spawn_blocking`.
struct DirScanResult {
    canonical: PathBuf,
    folders: Vec<BrowseFolder>,
    audio_paths: Vec<PathBuf>,
}

/// Classify one directory's entries into its visible sub-folders (name-sorted)
/// and its audio files. Dot-entries and anything whose type can't be read are
/// skipped; audio is decided by the one shared [`is_audio_extension`] predicate.
///
/// Split out of [`browse_directory`]'s blocking closure so the tests exercise
/// the shipped walk instead of a copy of it — they can't drive the closure
/// itself, which needs an `AppState` and the library-folder guard.
fn classify_dir_entries(
    dir: &std::path::Path,
) -> Result<(Vec<BrowseFolder>, Vec<PathBuf>), AppError> {
    let mut folders = Vec::new();
    let mut audio_paths = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if name.starts_with('.') {
            continue;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            folders.push(BrowseFolder {
                name: name.into_owned(),
                path: entry.path().to_string_lossy().into_owned(),
            });
        } else if file_type.is_file() {
            let entry_path = entry.path();
            if let Some(ext) = entry_path.extension().and_then(|e| e.to_str())
                && is_audio_extension(ext)
            {
                audio_paths.push(entry_path);
            }
        }
    }

    folders.sort_by_cached_key(|f| f.name.to_lowercase());

    Ok((folders, audio_paths))
}

/// Scan `path` for subfolders + audio files. `library_folders` is the
/// caller's already-fetched library-folder list — passed in (rather than
/// re-queried here) so a navigation does a single `folders` read shared
/// with the caller's `build_breadcrumbs`.
pub async fn browse_directory(
    state: &AppState,
    path: String,
    library_folders: &[Folder],
) -> Result<BrowseResult, AppError> {
    let enabled_canonical: Vec<PathBuf> = library_folders
        .iter()
        .filter(|f| f.is_enabled)
        .filter_map(|f| melodia_core::utils::canonicalize_path(&f.path).ok())
        .collect();

    let scan = tokio::task::spawn_blocking(move || -> Result<DirScanResult, AppError> {
        let canonical = melodia_core::utils::canonicalize_path(&path)
            .map_err(|_| AppError::Validation(format!("Path does not exist: {path}")))?;

        if !canonical.is_dir() {
            return Err(AppError::Validation(format!("Path is not a directory: {path}")));
        }

        let in_library = enabled_canonical.iter().any(|cp| canonical.starts_with(cp));
        if !in_library {
            return Err(AppError::Validation(
                "Path is not within an enabled library folder".to_owned(),
            ));
        }

        let (folders, audio_paths) = classify_dir_entries(&canonical)?;

        Ok(DirScanResult {
            canonical,
            folders,
            audio_paths,
        })
    })
    .await
    .map_err(AppError::io_source)??;

    let dir_str = scan.canonical.to_string_lossy();
    let tracks = queries::track::get_tracks_in_directory(&state.db, &dir_str).await?;
    // Move the DB rows into a path-keyed map (rather than borrow): the
    // `remove` below hands ownership straight to `BrowseFile`, so an
    // in-library file never deep-clones its ~18-field `TrackListRow` — the
    // only per-row clone is the `file_path` key (one `String` vs ~6 heap
    // allocs for a full row clone).
    let mut track_map: HashMap<String, TrackListRow> =
        tracks.into_iter().map(|t| (t.file_path.clone(), t)).collect();

    let mut files: Vec<BrowseFile> = Vec::with_capacity(scan.audio_paths.len());
    for audio_path in &scan.audio_paths {
        let path_str = audio_path.to_string_lossy();
        let file_name =
            audio_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

        if let Some(row) = track_map.remove(path_str.as_ref()) {
            files.push(BrowseFile {
                row,
                in_library: true,
            });
        } else {
            // Disk-only file: present on disk inside a library folder but
            // not (yet) in the DB (just copied, or scanner-rejected).
            // Synthesize a sparse `TrackListRow` with `id == 0` so the
            // shared `TrackList` can render it dimmed and non-interactive.
            files.push(BrowseFile {
                row: TrackListRow {
                    id: 0,
                    file_path: path_str.into_owned(),
                    file_name: file_name.clone(),
                    title: file_name,
                    artist: None,
                    album_artist: None,
                    album: None,
                    genre: None,
                    track_number: None,
                    disc_number: None,
                    year: None,
                    duration_ms: 0,
                    artwork_path: None,
                    is_favorite: false,
                    rating: 0,
                    album_id: None,
                    artist_id: None,
                    genre_id: None,
                    date_added: String::new(),
                    sort_key: None,
                },
                in_library: false,
            });
        }
    }

    files.sort_by_cached_key(|f| f.row.file_name.to_lowercase());

    let name = scan
        .canonical
        .file_name()
        .map_or_else(|| dir_str.to_string(), |n| n.to_string_lossy().into_owned());

    Ok(BrowseResult {
        path: dir_str.into_owned(),
        name,
        folders: scan.folders,
        files,
    })
}

#[cfg(test)]
#[path = "tests/browse_tests.rs"]
mod tests;
