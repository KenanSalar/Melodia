use std::path::PathBuf;

use tempfile::TempDir;

use super::*;

/// Drive the production walk ([`classify_dir_entries`]) behind the same
/// canonicalize + `is_dir` guards `browse_directory` applies, minus the
/// library-folder check (which needs an `AppState`). The classification itself
/// is the shipped code, not a copy of it.
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

    let (folders, audio_paths) = classify_dir_entries(&canonical)?;

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
