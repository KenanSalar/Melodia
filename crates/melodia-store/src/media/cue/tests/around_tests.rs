//! What opening a `.cue` opens: the audio files it names, in the sheet's order.

use std::fs;

use tempfile::TempDir;

use super::*;

const TRACK_AT_START: &str = "TRACK 01 AUDIO\nINDEX 01 00:00:00";

fn opened(tmp: &TempDir, sheets: &[&str]) -> Vec<Option<Vec<String>>> {
    let paths: Vec<PathBuf> = sheets.iter().map(|sheet| tmp.path().join(sheet)).collect();
    named_files(&paths)
        .into_iter()
        .map(|files| {
            let files = files.ok()?;
            let names =
                files.iter().filter_map(|file| file.file_name()?.to_str().map(str::to_owned));
            Some(names.collect())
        })
        .collect()
}

#[test]
fn a_sheet_opens_the_files_it_names_in_its_own_order() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    for file in ["01.flac", "02.flac"] {
        fs::write(tmp.path().join(file), b"")?;
    }
    let sheet =
        format!("FILE \"02.flac\" WAVE\n{TRACK_AT_START}\nFILE \"01.flac\" WAVE\n{TRACK_AT_START}");
    fs::write(tmp.path().join("rip.cue"), sheet)?;

    assert_eq!(
        opened(&tmp, &["rip.cue"]),
        [Some(vec!["02.flac".to_owned(), "01.flac".to_owned()])]
    );
    Ok(())
}

#[test]
fn a_file_named_twice_opens_once() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    for file in ["a.flac", "b.flac"] {
        fs::write(tmp.path().join(file), b"")?;
    }
    let sheet = ["a.flac", "b.flac", "a.flac"]
        .map(|file| format!("FILE \"{file}\" WAVE\n{TRACK_AT_START}"))
        .join("\n");
    fs::write(tmp.path().join("album.cue"), sheet)?;

    assert_eq!(
        opened(&tmp, &["album.cue"]),
        [Some(vec!["a.flac".to_owned(), "b.flac".to_owned()])]
    );
    Ok(())
}

/// Each sheet answers for itself, so one that opens nothing doesn't cost its neighbours theirs.
#[test]
fn a_sheet_naming_nothing_beside_it_or_placing_no_track_opens_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    fs::write(tmp.path().join("album.flac"), b"")?;
    fs::write(tmp.path().join("album.cue"), format!("FILE \"album.flac\" WAVE\n{TRACK_AT_START}"))?;
    fs::write(
        tmp.path().join("elsewhere.cue"),
        format!("FILE \"gone.flac\" WAVE\n{TRACK_AT_START}"),
    )?;
    fs::write(tmp.path().join("empty.cue"), "FILE \"album.flac\" WAVE")?;

    let opened = opened(&tmp, &["elsewhere.cue", "album.cue", "empty.cue"]);

    assert_eq!(opened, [None, Some(vec!["album.flac".to_owned()]), None]);
    Ok(())
}
