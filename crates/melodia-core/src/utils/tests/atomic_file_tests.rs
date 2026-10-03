use std::collections::BTreeSet;
use std::io::{Seek, SeekFrom};

use serde::Deserialize;

use super::*;

fn entries_in(dir: &Path) -> Result<BTreeSet<String>, AppError> {
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(dir)? {
        names.insert(entry?.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// A state file in the shapes a load meets: a token, a struct flattened into the top level as
/// `settings.json` flattens its flag structs, a nested struct and a list. Every default is a zero,
/// so a field that kept the file's value is told apart from one that reset.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Saved {
    volume: u32,
    style: Style,
    #[serde(flatten)]
    window: Window,
    geometry: Geometry,
    recent: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Style {
    #[default]
    Standard,
    Macos,
}

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Window {
    width: u32,
    maximized: bool,
}

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Geometry {
    x: i32,
    y: i32,
}

fn loaded(json: &str) -> Result<Saved, AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.json");
    std::fs::write(&path, json)?;
    load_json_or_default_sync(&path)
}

#[test]
fn a_file_this_build_wrote_reads_back_whole() -> Result<(), AppError> {
    let json = r#"{"volume": 40, "style": "macos", "width": 900, "maximized": true,
        "geometry": {"x": 10, "y": 20}, "recent": ["first"]}"#;

    let saved = loaded(json)?;

    let whole = Saved {
        volume: 40,
        style: Style::Macos,
        window: Window { width: 900, maximized: true },
        geometry: Geometry { x: 10, y: 20 },
        recent: vec!["first".to_owned()],
    };
    assert_eq!(saved, whole);
    Ok(())
}

/// The downgrade case: a token a newer build added. Read whole, the file defaulted and the next
/// write persisted that over every setting the user had.
#[test]
fn a_token_this_build_does_not_know_resets_that_field_alone() -> Result<(), AppError> {
    let saved = loaded(r#"{"volume": 40, "style": "a_newer_style", "width": 900}"#)?;

    let expected =
        Saved { volume: 40, window: Window { width: 900, maximized: false }, ..Saved::default() };
    assert_eq!(saved, expected);
    Ok(())
}

#[test]
fn a_field_whose_type_changed_resets_alone() -> Result<(), AppError> {
    let saved = loaded(r#"{"volume": true, "style": "macos"}"#)?;

    assert_eq!(saved, Saved { style: Style::Macos, ..Saved::default() });
    Ok(())
}

/// A flattened struct's fields sit at the top level, which is where every setting is, so one that
/// won't read leaves the rest of its struct standing.
#[test]
fn a_flattened_field_that_wont_read_resets_alone() -> Result<(), AppError> {
    let saved = loaded(r#"{"width": "wide", "maximized": true}"#)?;

    assert_eq!(saved.window, Window { width: 0, maximized: true });
    Ok(())
}

/// Below the top level the field holding the value is what resets, the struct or list whole.
#[test]
fn a_value_nested_deeper_resets_the_field_holding_it() -> Result<(), AppError> {
    let rows = [
        ("a struct", r#"{"volume": 40, "geometry": {"x": "left", "y": 20}}"#),
        ("a list", r#"{"volume": 40, "recent": ["first", 7]}"#),
    ];
    for (what, json) in rows {
        let saved = loaded(json)?;

        assert_eq!(saved, Saved { volume: 40, ..Saved::default() }, "{what}");
    }
    Ok(())
}

#[test]
fn every_value_that_wont_read_resets_and_the_rest_still_reads() -> Result<(), AppError> {
    let json = r#"{"volume": "loud", "style": "a_newer_style", "width": -5, "maximized": true,
        "recent": ["first"]}"#;

    let saved = loaded(json)?;

    let expected = Saved {
        window: Window { width: 0, maximized: true },
        recent: vec!["first".to_owned()],
        ..Saved::default()
    };
    assert_eq!(saved, expected);
    Ok(())
}

/// Nothing to salvage from a file that isn't JSON, nor from one that isn't an object at the top.
#[test]
fn a_file_with_nothing_to_salvage_falls_back_whole() -> Result<(), AppError> {
    let rows = [("not JSON", r#"{"volume": 40,"#), ("a list", "[40]"), ("a number", "40")];
    for (what, json) in rows {
        let saved = loaded(json)?;

        assert_eq!(saved, Saved::default(), "{what}");
    }
    Ok(())
}

#[tokio::test]
async fn the_async_load_resets_a_value_alone_too() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("state.json");
    std::fs::write(&path, r#"{"volume": 40, "style": "a_newer_style"}"#)?;

    let saved: Saved = load_json_or_default(&path).await?;

    assert_eq!(saved, Saved { volume: 40, ..Saved::default() });
    Ok(())
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
