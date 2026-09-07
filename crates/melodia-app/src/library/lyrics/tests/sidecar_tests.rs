//! Which file beside the track counts as its sheet, and what a missing one means.

use super::*;

/// A track file that exists, so the sidecar lookup has somewhere to look beside.
fn staged_track(tmp: &tempfile::TempDir) -> Result<PathBuf, AppError> {
    let audio = tmp.path().join("song.mp3");
    std::fs::write(&audio, b"not really audio")?;
    Ok(audio)
}

fn names(path: &str) -> Vec<String> {
    candidates(Path::new(path))
        .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn both_sidecar_shapes_are_looked_for() {
    // Both are in circulation and which one a user has depends on whatever produced it, so
    // checking a single name is a coin flip.
    assert_eq!(names("/m/song.mp3"), vec!["song.lrc", "song.mp3.lrc"]);
}

#[test]
fn a_name_with_dots_in_it_still_yields_both_shapes() {
    assert_eq!(names("/m/a.b.flac"), vec!["a.b.lrc", "a.b.flac.lrc"]);
}

#[test]
fn a_track_with_no_extension_is_survivable() {
    // The two shapes collapse onto one name, which costs a repeated look and nothing else.
    assert_eq!(names("/m/noext"), vec!["noext.lrc", "noext.lrc"]);
}

#[test]
fn no_sidecar_is_no_sheet_and_no_error() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    let audio = staged_track(&tmp)?;

    assert!(read(&audio)?.is_none(), "a missing sidecar is the common case, not a failure");
    Ok(())
}

#[test]
fn the_appended_shape_is_read() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    let audio = staged_track(&tmp)?;
    std::fs::write(tmp.path().join("song.mp3.lrc"), "[00:01.00]appended shape\n")?;

    let found = read(&audio)?.ok_or_else(|| AppError::Validation("no sheet".into()))?;
    assert_eq!(found.lines[0].text, "appended shape");
    Ok(())
}

#[test]
fn the_stem_shape_outranks_the_appended_one() -> Result<(), AppError> {
    // Order is the contract: the first name checked is the one that answers.
    let tmp = tempfile::TempDir::new()?;
    let audio = staged_track(&tmp)?;
    std::fs::write(tmp.path().join("song.mp3.lrc"), "[00:01.00]appended\n")?;
    std::fs::write(tmp.path().join("song.lrc"), "[00:01.00]stem\n")?;

    let found = read(&audio)?.ok_or_else(|| AppError::Validation("no sheet".into()))?;
    assert_eq!(found.lines[0].text, "stem");
    Ok(())
}

#[test]
fn a_sidecar_is_marked_as_one() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    let audio = staged_track(&tmp)?;
    std::fs::write(tmp.path().join("song.lrc"), "plain words\n")?;

    let found = read(&audio)?.ok_or_else(|| AppError::Validation("no sheet".into()))?;
    assert_eq!(found.source, LyricsSource::Sidecar);
    assert!(!found.is_synced(), "an untimed sidecar is still a sidecar");
    Ok(())
}
