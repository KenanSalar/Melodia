use std::collections::BTreeSet;
use std::io::{Seek, SeekFrom};

use super::*;

fn entries_in(dir: &Path) -> Result<BTreeSet<String>, AppError> {
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(dir)? {
        names.insert(entry?.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// The rename is what keeps a reader from ever seeing half a settings file, so a write that fails
/// part way has to leave the one already there whole.
#[test]
fn a_failed_write_leaves_the_existing_file_as_it_was() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{\"theme\":\"mocha\"}")?;

    let failed = write_with_sync(&path, |writer| {
        writer.write_all(b"{\"theme\":")?;
        Err(AppError::io_other("serializer gave up"))
    });

    assert!(failed.is_err());
    assert_eq!(std::fs::read_to_string(&path)?, "{\"theme\":\"mocha\"}");
    Ok(())
}

#[test]
fn a_failed_write_leaves_no_temp_file_behind() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{}")?;

    let failed = write_with_sync(&path, |_| Err(AppError::io_other("serializer gave up")));

    assert!(failed.is_err());
    assert_eq!(entries_in(dir.path())?, BTreeSet::from(["settings.json".to_owned()]));
    Ok(())
}

#[test]
fn a_write_creates_the_folders_leading_to_its_file() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("lyrics").join("cache").join("sheet.lrc");

    write_text_sync(&path, "[00:01.00]first line")?;

    assert_eq!(std::fs::read_to_string(&path)?, "[00:01.00]first line");
    Ok(())
}

#[test]
fn a_write_replaces_the_file_already_there() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("queue.json");
    std::fs::write(&path, "an older and much longer queue than the one replacing it")?;

    write_text_sync(&path, "[]")?;

    assert_eq!(std::fs::read_to_string(&path)?, "[]");
    Ok(())
}

/// A zip goes back over each entry's header once it knows the entry's size.
#[test]
fn the_writer_can_go_back_over_what_it_wrote() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("backup.zip");

    write_with_sync(&path, |writer| {
        writer.write_all(b"abcd")?;
        writer.seek(SeekFrom::Start(1))?;
        writer.write_all(b"X")?;
        Ok(())
    })?;

    assert_eq!(std::fs::read_to_string(&path)?, "aXcd");
    Ok(())
}
