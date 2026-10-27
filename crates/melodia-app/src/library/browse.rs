use std::collections::HashMap;
use std::path::PathBuf;

use melodia_core::entities::browse::{BrowseFile, BrowseFolder, BrowseResult};
use melodia_core::entities::folder::Folder;
use melodia_core::entities::track::TrackListRow;
use melodia_core::error::AppError;
use melodia_core::utils::audio_ext::is_audio_extension;
use melodia_store::database::{DbPool, queries};

struct DirScanResult {
    canonical: PathBuf,
    folders: Vec<BrowseFolder>,
    audio_paths: Vec<PathBuf>,
}

/// Dot-entries and anything whose type can't be read are skipped.
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

/// `library_folders` is passed in, so a navigation shares one read with the caller's
/// `build_breadcrumbs`.
pub async fn browse_directory(
    db: &DbPool,
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
            .map_err(|e| AppError::io(format!("Cannot resolve path {path}"), e))?;

        if !canonical.is_dir() {
            return Err(AppError::validation_msg(format!("Path is not a directory: {path}")));
        }

        let in_library = enabled_canonical.iter().any(|cp| canonical.starts_with(cp));
        if !in_library {
            return Err(AppError::validation_msg("Path is not within an enabled library folder"));
        }

        let (folders, audio_paths) = classify_dir_entries(&canonical)?;

        Ok(DirScanResult { canonical, folders, audio_paths })
    })
    .await
    .map_err(AppError::io_source)??;

    let dir_str = scan.canonical.to_string_lossy();
    let tracks = queries::track::get_tracks_in_directory(db, &dir_str).await?;
    let mut track_map: HashMap<String, TrackListRow> =
        tracks.into_iter().map(|t| (t.file_path, t.row)).collect();

    let mut files: Vec<BrowseFile> = Vec::with_capacity(scan.audio_paths.len());
    for audio_path in &scan.audio_paths {
        let path_str = audio_path.to_string_lossy();
        let file_name =
            audio_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

        if let Some(row) = track_map.remove(path_str.as_ref()) {
            files.push(BrowseFile { row, file_name, in_library: true });
        } else {
            // On disk but not in the database, just copied or refused by the scanner: an `id == 0`
            // row the shared `TrackList` renders dimmed and inert.
            files.push(BrowseFile {
                row: TrackListRow {
                    id: 0,
                    title: file_name.clone(),
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
                    sort_key: None,
                },
                file_name,
                in_library: false,
            });
        }
    }

    files.sort_by_cached_key(|f| f.file_name.to_lowercase());

    let name = scan
        .canonical
        .file_name()
        .map_or_else(|| dir_str.to_string(), |n| n.to_string_lossy().into_owned());

    Ok(BrowseResult { path: dir_str.into_owned(), name, folders: scan.folders, files })
}

/// Every track under `path`, subdirectories included — what a folder card's Play, Play Next and
/// Add to Queue act on.
///
/// **Recursive, unlike what the view lists for the folder it is standing in.** A folder card is a
/// folder the user has *not* navigated into, so an artist folder holding only album subfolders
/// would otherwise offer a Play that queues nothing.
pub async fn folder_track_ids(db: &DbPool, path: &str) -> Result<Vec<i64>, AppError> {
    queries::track::track_ids_under_directory(db, path).await
}

#[cfg(test)]
#[path = "tests/browse_tests.rs"]
mod tests;
