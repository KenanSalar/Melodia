//! Tests for the shipped browse walk, driven through [`list_directory`] itself.
//!
//! They used to drive a local copy of the canonicalize + `is_dir` ladder, because the entry point
//! took an `&AppState`. The copy carried no library-folder guard, so the half of the walk that
//! decides whether a path may be browsed at all was never run.

use std::path::Path;

use tempfile::TempDir;

use super::*;
use melodia_store::database::queries::fixtures::insert_test_track;

/// A library folder row for `dir`, as `browse_directory`'s caller hands them in.
fn library_folder(dir: &Path, is_enabled: bool) -> Folder {
    Folder {
        id: 1,
        path: dir.to_string_lossy().into_owned(),
        is_enabled,
        last_scanned: None,
        added_at: String::new(),
    }
}

/// The canonical spelling of `dir`, which is what the walk matches rows against — a temp root is
/// often a symlink, so the path a test seeds has to be the resolved one.
fn resolved(dir: &Path) -> Result<PathBuf, AppError> {
    melodia_core::utils::canonicalize_path(dir)
        .map_err(|e| AppError::Validation(format!("resolve {}: {e}", dir.display())))
}

async fn browse(db: &DbPool, dir: &Path, folders: &[Folder]) -> Result<BrowseResult, AppError> {
    list_directory(db, dir.to_string_lossy().into_owned(), folders).await
}

#[tokio::test]
async fn a_path_that_does_not_exist_is_refused() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let missing = PathBuf::from("/nonexistent/browse/path");
    let folders = [library_folder(&missing, true)];

    let Err(err) = browse(&db, &missing, &folders).await else {
        return Err(AppError::Validation("a missing path must not browse".into()));
    };
    assert!(err.to_string().contains("does not exist"), "got: {err}");
    Ok(())
}

#[tokio::test]
async fn a_file_is_not_a_directory_to_browse() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let file = tmp.path().join("not_a_dir.txt");
    std::fs::write(&file, "data")?;
    let folders = [library_folder(tmp.path(), true)];

    let Err(err) = browse(&db, &file, &folders).await else {
        return Err(AppError::Validation("a file must not browse".into()));
    };
    assert!(err.to_string().contains("not a directory"), "got: {err}");
    Ok(())
}

/// The guard the old test-local copy could not reach. Browse is a view onto the library, not a
/// file manager, so a directory nobody added is not somewhere it will list.
#[tokio::test]
async fn a_path_outside_every_library_folder_is_refused() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let outside = TempDir::new()?;
    let folders = [library_folder(tmp.path(), true)];

    let Err(err) = browse(&db, outside.path(), &folders).await else {
        return Err(AppError::Validation("a path outside the library must not browse".into()));
    };
    assert!(err.to_string().contains("enabled library folder"), "got: {err}");
    Ok(())
}

/// The same guard from the other side: a folder the user switched off is not a folder to browse,
/// and disabling one is how they hide it.
#[tokio::test]
async fn a_path_inside_a_disabled_library_folder_is_refused() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let folders = [library_folder(tmp.path(), false)];

    let Err(err) = browse(&db, tmp.path(), &folders).await else {
        return Err(AppError::Validation("a disabled folder must not browse".into()));
    };
    assert!(err.to_string().contains("enabled library folder"), "got: {err}");
    Ok(())
}

#[tokio::test]
async fn a_dot_entry_is_not_browsed() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    std::fs::write(tmp.path().join(".hidden.mp3"), "data")?;
    std::fs::write(tmp.path().join("visible.mp3"), "data")?;
    std::fs::create_dir(tmp.path().join(".hidden_dir"))?;
    std::fs::create_dir(tmp.path().join("visible_dir"))?;
    let folders = [library_folder(tmp.path(), true)];

    let result = browse(&db, tmp.path(), &folders).await?;
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.folders.len(), 1);
    assert_eq!(result.folders[0].name, "visible_dir");
    Ok(())
}

#[tokio::test]
async fn only_audio_files_and_folders_are_listed() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    for name in [
        "song.mp3",
        "track.flac",
        "clip.ogg",
        "cover.jpg",
        "notes.txt",
    ] {
        std::fs::write(tmp.path().join(name), "data")?;
    }
    std::fs::create_dir(tmp.path().join("Rock"))?;
    std::fs::create_dir(tmp.path().join("Jazz"))?;
    let folders = [library_folder(tmp.path(), true)];

    let result = browse(&db, tmp.path(), &folders).await?;
    assert_eq!(result.files.len(), 3, "the cover and the notes are not audio");
    assert_eq!(result.folders.len(), 2);
    Ok(())
}

#[tokio::test]
async fn folders_and_files_sort_case_insensitively() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    for name in ["Zebra", "alpha", "Beta"] {
        std::fs::create_dir(tmp.path().join(name))?;
    }
    for name in ["Zulu.mp3", "alpha.mp3", "Bravo.mp3"] {
        std::fs::write(tmp.path().join(name), "data")?;
    }
    let folders = [library_folder(tmp.path(), true)];

    let result = browse(&db, tmp.path(), &folders).await?;
    let folder_names: Vec<&str> = result.folders.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(folder_names, ["alpha", "Beta", "Zebra"]);
    let file_names: Vec<&str> = result.files.iter().map(|f| f.row.file_name.as_str()).collect();
    assert_eq!(file_names, ["alpha.mp3", "Bravo.mp3", "Zulu.mp3"]);
    Ok(())
}

/// The path-keyed map's whole job. A file the library knows carries its row; one that is merely
/// on disk is synthesized with `id == 0`, which is what the shared `TrackList` dims and refuses
/// to play — handing it a real-looking id would offer the user a row nothing can load.
#[tokio::test]
async fn a_file_carries_its_row_only_when_the_library_holds_one() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let dir = resolved(tmp.path())?;
    queries::folder::insert_folder(&db, &dir.to_string_lossy(), true).await?;

    let known = dir.join("known.mp3");
    std::fs::write(&known, "data")?;
    std::fs::write(dir.join("unknown.mp3"), "data")?;
    insert_test_track(&db, &known.to_string_lossy(), "Known Song", "Artist", "Album", "Rock")
        .await?;

    let folders = [library_folder(&dir, true)];
    let result = browse(&db, &dir, &folders).await?;

    let listed: Vec<(&str, bool, i64, &str)> = result
        .files
        .iter()
        .map(|f| (f.row.file_name.as_str(), f.in_library, f.row.id, f.row.title.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("known.mp3", true, 1, "Known Song"),
            ("unknown.mp3", false, 0, "unknown.mp3")
        ],
        "a disk-only file must be titled by its name and carry no id"
    );
    Ok(())
}
