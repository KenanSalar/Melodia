//! Which disabled folders the boot sweep may treat as outside the library.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use melodia_core::utils::canonicalize_path;

/// A library folder holding a file, so it reads as mounted, and the import nested in it.
fn online_library() -> Result<(TempDir, PathBuf, PathBuf), AppError> {
    let tmp = TempDir::new()?;
    let root = canonicalize_path(tmp.path())?;
    let nested = root.join("Downloads");
    fs::create_dir(&nested)?;
    fs::write(root.join("song.flac"), b"")?;
    Ok((tmp, root, nested))
}

#[test]
fn a_folder_inside_a_library_folder_awaits_its_absorb() -> Result<(), AppError> {
    let (_tmp, root, nested) = online_library()?;
    assert!(awaits_absorb(&nested, &[&root]));
    Ok(())
}

#[test]
fn a_folder_outside_every_library_folder_is_swept() -> Result<(), AppError> {
    let (_tmp, root, _) = online_library()?;
    let elsewhere = TempDir::new()?;

    assert!(!awaits_absorb(elsewhere.path(), &[&root]));
    Ok(())
}

#[test]
fn a_gone_folder_under_an_online_library_folder_is_swept() -> Result<(), AppError> {
    let (_tmp, root, nested) = online_library()?;
    fs::remove_dir(&nested)?;

    assert!(!awaits_absorb(&nested, &[&root]));
    Ok(())
}

#[test]
fn a_gone_folder_under_an_offline_library_folder_awaits_its_absorb() -> Result<(), AppError> {
    let mount_point = TempDir::new()?;
    let nested = mount_point.path().join("Downloads");

    assert!(awaits_absorb(&nested, &[mount_point.path()]));
    Ok(())
}

#[test]
fn a_gone_folder_under_no_library_folder_is_swept() -> Result<(), AppError> {
    let mount_point = TempDir::new()?;
    let elsewhere = TempDir::new()?;
    let gone = elsewhere.path().join("Downloads");

    assert!(!awaits_absorb(&gone, &[mount_point.path(), Path::new("/no/such/library")]));
    Ok(())
}
