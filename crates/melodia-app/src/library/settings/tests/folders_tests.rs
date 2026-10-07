use std::collections::HashSet;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

use super::{folders_inside, validate_folder_path, watch_roots};

fn make_folder(id: i64, path: &str) -> Folder {
    Folder {
        id,
        path: path.to_owned(),
        is_enabled: true,
        last_scanned: None,
        added_at: String::new(),
    }
}

fn err_msg<T>(result: Result<T, AppError>) -> Result<String, AppError> {
    let Err(e) = result else {
        return Err(AppError::Validation("expected error".into()));
    };
    Ok(e.to_string())
}

#[test]
fn validate_nonexistent_path_returns_error() -> Result<(), AppError> {
    let result = validate_folder_path(Path::new("/nonexistent/path/xyz"), &[]);
    let msg = err_msg(result)?;
    assert!(msg.contains("does not exist"), "got: {msg}");
    Ok(())
}

#[test]
fn validate_file_not_dir_returns_error() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let file_path = tmp.path().join("file.txt");
    std::fs::write(&file_path, "hello")?;

    let result = validate_folder_path(&file_path, &[]);
    let msg = err_msg(result)?;
    assert!(msg.contains("not a directory"), "got: {msg}");
    Ok(())
}

#[test]
fn validate_duplicate_folder_returns_error() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let dir = tmp.path().join("music");
    std::fs::create_dir(&dir)?;

    let canonical = melodia_core::utils::canonicalize_path(&dir)?;
    let canonical_str =
        canonical.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?;
    let existing = make_folder(1, canonical_str);

    let result = validate_folder_path(&dir, &[existing]);
    let msg = err_msg(result)?;
    assert!(msg.contains("already in your library"), "got: {msg}");
    Ok(())
}

#[test]
fn validate_child_of_existing_returns_error() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let parent = tmp.path().join("music");
    let child = parent.join("rock");
    std::fs::create_dir_all(&child)?;

    let canonical_parent = melodia_core::utils::canonicalize_path(&parent)?;
    let canonical_str =
        canonical_parent.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?;
    let existing = make_folder(1, canonical_str);

    let result = validate_folder_path(&child, &[existing]);
    let msg = err_msg(result)?;
    assert!(msg.contains("already covered by"), "got: {msg}");
    Ok(())
}

#[test]
fn folders_inside_finds_every_folder_under_the_path() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let parent = tmp.path().join("music");
    let child1 = parent.join("rock");
    let child2 = parent.join("jazz");
    std::fs::create_dir_all(&child1)?;
    std::fs::create_dir_all(&child2)?;

    let c1 = melodia_core::utils::canonicalize_path(&child1)?;
    let c2 = melodia_core::utils::canonicalize_path(&child2)?;
    let existing = vec![
        make_folder(10, c1.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?),
        make_folder(20, c2.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?),
    ];

    let canonical_parent = melodia_core::utils::canonicalize_path(&parent)?;
    let mut ids: Vec<i64> =
        folders_inside(&canonical_parent, &existing).iter().map(|f| f.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [10, 20]);
    Ok(())
}

#[test]
fn folders_inside_leaves_out_an_unrelated_folder() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let dir_a = tmp.path().join("music_a");
    let dir_b = tmp.path().join("music_b");
    std::fs::create_dir(&dir_a)?;
    std::fs::create_dir(&dir_b)?;

    let canonical_a = melodia_core::utils::canonicalize_path(&dir_a)?;
    let existing = make_folder(
        1,
        canonical_a.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?,
    );

    let canonical_b = melodia_core::utils::canonicalize_path(&dir_b)?;
    assert!(folders_inside(&canonical_b, &[existing]).is_empty());
    Ok(())
}

/// A scan asks this of every folder, its own row among them. Counted as nested, it would read
/// none of its own files and then absorb itself, the delete taking every track with it.
#[test]
fn folders_inside_leaves_out_the_folder_itself() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = tmp.path().join("music");
    std::fs::create_dir(&music)?;
    let canonical = melodia_core::utils::canonicalize_path(&music)?;
    let own_row = make_folder(
        1,
        canonical.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?,
    );

    assert!(folders_inside(&canonical, &[own_row]).is_empty());
    Ok(())
}

/// An unmounted drive, say. Absorbed, its tracks would belong to a folder whose next walk can't
/// find them, and that walk's purge would delete them.
#[test]
fn folders_inside_leaves_out_a_folder_whose_directory_is_gone() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = tmp.path().join("music");
    std::fs::create_dir(&music)?;
    let canonical = melodia_core::utils::canonicalize_path(&music)?;
    let unmounted = canonical.join("external");
    let row = make_folder(
        2,
        unmounted.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?,
    );

    assert!(folders_inside(&canonical, &[row]).is_empty());
    Ok(())
}

/// A recursive watch on a folder already covers the folders inside it, and unwatching one of
/// those once it is absorbed would take the outer folder's watches with it.
#[tokio::test]
async fn the_watcher_follows_only_the_outermost_enabled_folders() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let music = tmp.path().join("music");
    let nested = music.join("rock");
    let beside = tmp.path().join("podcasts");
    let disabled = tmp.path().join("imported");
    for (path, is_enabled) in [(&music, true), (&nested, true), (&beside, true), (&disabled, false)]
    {
        queries::folder::insert_folder(&db, &path.to_string_lossy(), is_enabled).await?;
    }

    let roots: HashSet<PathBuf> = watch_roots(&db).await?.into_iter().collect();

    assert_eq!(roots, HashSet::from([music, beside]));
    Ok(())
}

#[test]
fn validate_deleted_existing_folder_skipped() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let dir = tmp.path().join("music");
    std::fs::create_dir(&dir)?;

    let existing = make_folder(1, "/nonexistent/deleted/folder");

    assert!(validate_folder_path(&dir, &[existing]).is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn validate_canonicalizes_symlinks() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let real_dir = tmp.path().join("real_music");
    std::fs::create_dir(&real_dir)?;

    let link_path = tmp.path().join("link_music");
    symlink(&real_dir, &link_path)?;

    let canonical = melodia_core::utils::canonicalize_path(&real_dir)?;
    let canonical_str =
        canonical.to_str().ok_or_else(|| AppError::Validation("non-utf8 path".into()))?;
    let existing = make_folder(1, canonical_str);

    let result = validate_folder_path(&link_path, &[existing]);
    let msg = err_msg(result)?;
    assert!(msg.contains("already in your library"), "got: {msg}");
    Ok(())
}
