//! The duplicate check that decides whether a first launch adds `~/Music` a second time.
//!
//! The rest of `run` wants a real `AppState` and reads `dirs::audio_dir()` off the environment,
//! so this is the half that can be asked honestly. It is also the half with a cost: a candidate
//! that fails to match a row already naming the same directory scans the user's whole library
//! again and leaves two rows in the Library settings list pointing at one folder.

use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;

fn folder_at(path: &Path) -> Folder {
    Folder {
        id: 1,
        path: path.to_string_lossy().into_owned(),
        is_enabled: true,
        last_scanned: None,
        added_at: "2026-01-01T00:00:00+00:00".to_owned(),
    }
}

#[test]
fn a_folder_already_in_the_library_is_recognised() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = melodia_core::utils::canonicalize_path(tmp.path())?;

    assert!(already_watched(&[folder_at(&music)], &music));
    Ok(())
}

#[test]
fn a_library_with_no_folders_recognises_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = melodia_core::utils::canonicalize_path(tmp.path())?;

    assert!(!already_watched(&[], &music));
    Ok(())
}

/// The resolution is the whole point of the function, so the case has to be a spelling `Path`
/// equality gets wrong. A trailing separator is not one: `Path` compares components and drops it,
/// so a version doing no resolution at all passes that. A traversal is, and it stands in for the
/// case that actually happens and cannot be written portably, a home directory reached through a
/// symlink on one side and not the other.
#[test]
fn the_same_directory_spelled_two_ways_is_still_the_same_directory() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = melodia_core::utils::canonicalize_path(tmp.path())?;
    let inner = music.join("inner");
    std::fs::create_dir(&inner)?;
    let stored = inner.join("..");

    assert!(
        already_watched(&[folder_at(&stored)], &music),
        "`{}` resolves to `{}`, so adding it again would index the library twice",
        stored.display(),
        music.display()
    );
    Ok(())
}

/// A folder the user has since deleted or unplugged is not a reason to skip the auto-add: the
/// candidate exists, this row does not, and `canonicalize_path` fails on it rather than
/// answering. Swallowing that as a match would leave a first launch with no library at all.
#[test]
fn a_row_whose_folder_is_gone_matches_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = melodia_core::utils::canonicalize_path(tmp.path())?;
    let unplugged = music.join("removable");

    assert!(!already_watched(&[folder_at(&unplugged)], &music));
    Ok(())
}
