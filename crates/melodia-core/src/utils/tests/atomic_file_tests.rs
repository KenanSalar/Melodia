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

/// A tag write routinely targets a track a deck is holding open, so a rewrite that fails part way
/// has to leave the file whole rather than half-edited under its reader.
#[test]
fn a_rewrite_that_fails_leaves_the_file_as_it_was() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("track.mp3");
    std::fs::write(&path, "original bytes")?;

    let failed = rewrite_with_sync(&path, |target| {
        std::fs::write(target, "half an edit")?;
        Err(AppError::io_other("the tag writer gave up"))
    });

    assert!(failed.is_err());
    assert_eq!(std::fs::read_to_string(&path)?, "original bytes");
    Ok(())
}

/// The temp carries no extension, so one left behind in a music folder is a file the scanner walks
/// past and the user never sees.
#[test]
fn a_rewrite_that_fails_leaves_no_temp_file_behind() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("track.mp3");
    std::fs::write(&path, "original bytes")?;

    let failed = rewrite_with_sync(&path, |_| Err(AppError::io_other("the tag writer gave up")));

    assert!(failed.is_err());
    assert_eq!(entries_in(dir.path())?, BTreeSet::from(["track.mp3".to_owned()]));
    Ok(())
}

/// A rename is only atomic within one filesystem, so the copy has to be made beside the file it
/// replaces rather than in the system temp directory, which is routinely a mount of its own.
#[test]
fn the_copy_is_written_beside_the_file_it_replaces() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("track.mp3");
    std::fs::write(&path, "original bytes")?;
    let resolved = std::fs::canonicalize(&path)?;

    let mut wrote_into = None;
    rewrite_with_sync(&path, |target| {
        wrote_into = target.parent().map(Path::to_path_buf);
        Ok(())
    })?;

    assert_eq!(wrote_into.as_deref(), resolved.parent());
    Ok(())
}

/// The caller opens the path for itself and rewrites the whole file, so it has to be handed the
/// content rather than an empty temp with the right name.
#[test]
fn a_rewrite_hands_the_closure_the_bytes_it_started_with() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("track.mp3");
    std::fs::write(&path, "original bytes")?;

    let mut handed = None;
    rewrite_with_sync(&path, |target| {
        handed = Some(std::fs::read_to_string(target)?);
        Ok(())
    })?;

    assert_eq!(handed.as_deref(), Some("original bytes"));
    Ok(())
}

/// **The `canonicalize` call's whole reason.** A rename replaces the *link* where a write in place
/// followed it, so a symlinked track would become a regular file holding the edit and the file the
/// user actually keeps would be left untouched.
#[cfg(unix)]
#[test]
fn a_rewrite_replaces_the_file_the_link_points_at_rather_than_the_link() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let kept = dir.path().join("track.mp3");
    let link = dir.path().join("linked.mp3");
    std::fs::write(&kept, "original bytes")?;
    std::os::unix::fs::symlink(&kept, &link)?;

    rewrite_with_sync(&link, |target| Ok(std::fs::write(target, "edited bytes")?))?;

    assert_eq!(std::fs::read_to_string(&kept)?, "edited bytes", "the edit missed the kept file");
    assert!(
        std::fs::symlink_metadata(&link)?.file_type().is_symlink(),
        "the link was replaced by a regular file"
    );
    Ok(())
}
