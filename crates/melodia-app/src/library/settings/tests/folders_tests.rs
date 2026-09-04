#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;

use tempfile::TempDir;

use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;

use super::validate_folder_path;

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
fn validate_parent_of_existing_returns_children_to_remove() -> Result<(), AppError> {
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

    let result = validate_folder_path(&parent, &existing)?;
    assert_eq!(result.len(), 2);
    assert!(result.contains(&10));
    assert!(result.contains(&20));
    Ok(())
}

#[test]
fn validate_unrelated_path_returns_empty() -> Result<(), AppError> {
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

    let result = validate_folder_path(&dir_b, &[existing])?;
    assert!(result.is_empty());
    Ok(())
}

#[test]
fn validate_deleted_existing_folder_skipped() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let dir = tmp.path().join("music");
    std::fs::create_dir(&dir)?;

    let existing = make_folder(1, "/nonexistent/deleted/folder");

    let result = validate_folder_path(&dir, &[existing])?;
    assert!(result.is_empty());
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

// `corner_radius_by_desktop_environment` was moved alongside
// `get_os_corner_radius` to `src/services/tests/settings_tests.rs`
// when those OS / desktop helpers moved from `library` to `services`
// (services owns serde defaults, library shouldn't know about env vars).
