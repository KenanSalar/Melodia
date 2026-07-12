use std::path::PathBuf;

use tempfile::TempDir;

use crate::entities::browse::BrowseFolder;
use crate::media::is_audio_extension;

use super::*;

/// Helper: replicate the directory scanning logic from `browse_directory`'s `spawn_blocking` closure.
/// This allows testing the filesystem classification without a full `AppState`.
fn scan_directory(path: &std::path::Path) -> Result<DirScanResult, AppError> {
    let canonical = crate::utils::canonicalize_path(path).map_err(|_| {
        AppError::Validation(format!("Path does not exist: {}", path.display()))
    })?;

    if !canonical.is_dir() {
        return Err(AppError::Validation(format!(
            "Path is not a directory: {}",
            path.display()
        )));
    }

    let mut folders = Vec::new();
    let mut audio_paths = Vec::new();

    for entry in std::fs::read_dir(&canonical)? {
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

    Ok(DirScanResult {
        canonical,
        folders,
        audio_paths,
    })
}

#[test]
fn scan_nonexistent_path_returns_error() -> Result<(), AppError> {
    let result = scan_directory(&PathBuf::from("/nonexistent/browse/path"));
    let Err(err) = result else {
        return Err(AppError::Validation("expected error".into()));
    };
    assert!(err.to_string().contains("does not exist"), "got: {err}");
    Ok(())
}

#[test]
fn scan_file_not_dir_returns_error() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let file = tmp.path().join("not_a_dir.txt");
    std::fs::write(&file, "data")?;

    let result = scan_directory(&file);
    let Err(err) = result else {
        return Err(AppError::Validation("expected error".into()));
    };
    assert!(err.to_string().contains("not a directory"), "got: {err}");
    Ok(())
}

#[test]
fn scan_skips_hidden_entries() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let dir = tmp.path().join("music");
    std::fs::create_dir(&dir)?;

    std::fs::write(dir.join(".hidden.mp3"), "data")?;
    std::fs::write(dir.join("visible.mp3"), "data")?;
    std::fs::create_dir(dir.join(".hidden_dir"))?;
    std::fs::create_dir(dir.join("visible_dir"))?;

    let result = scan_directory(&dir)?;
    assert_eq!(result.audio_paths.len(), 1);
    assert_eq!(result.folders.len(), 1);
    assert_eq!(result.folders[0].name, "visible_dir");
    Ok(())
}

#[test]
fn scan_classifies_folders_and_audio_files() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let dir = tmp.path().join("library");
    std::fs::create_dir(&dir)?;

    std::fs::write(dir.join("song.mp3"), "data")?;
    std::fs::write(dir.join("track.flac"), "data")?;
    std::fs::write(dir.join("clip.ogg"), "data")?;

    std::fs::write(dir.join("cover.jpg"), "data")?;
    std::fs::write(dir.join("notes.txt"), "data")?;

    std::fs::create_dir(dir.join("Rock"))?;
    std::fs::create_dir(dir.join("Jazz"))?;

    let result = scan_directory(&dir)?;
    assert_eq!(result.audio_paths.len(), 3);
    assert_eq!(result.folders.len(), 2);
    Ok(())
}

#[test]
fn scan_sorts_folders_case_insensitive() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let dir = tmp.path().join("sorted");
    std::fs::create_dir(&dir)?;

    std::fs::create_dir(dir.join("Zebra"))?;
    std::fs::create_dir(dir.join("alpha"))?;
    std::fs::create_dir(dir.join("Beta"))?;

    let result = scan_directory(&dir)?;
    let names: Vec<&str> = result.folders.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "Beta", "Zebra"]);
    Ok(())
}
