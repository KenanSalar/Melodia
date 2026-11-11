//! Which walked audio file a sheet's `FILE` line means: as written, then folded, then by stem,
//! with the rippers' Windows spellings read as paths.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::media::ingest::scanner::{Unobserved, collect_media_files};
use melodia_core::error::AppError;

fn walked(files: &[&str]) -> Result<(TempDir, WalkIndex), AppError> {
    let tmp = TempDir::new()?;
    for file in files {
        let path = tmp.path().join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, b"")?;
    }
    let walk = collect_media_files(tmp.path(), &Unobserved)
        .ok_or_else(|| AppError::validation_msg("a walk nothing cancelled came back stopped"))?;
    Ok((tmp, WalkIndex::new(walk.files, walk.unreadable)))
}

fn resolved(name: &str, root: &Path, sheet_dir: &str, audio: &WalkIndex) -> Option<PathBuf> {
    let found = resolve(name, &root.join(sheet_dir), audio)?;
    found.strip_prefix(root).ok().map(Path::to_path_buf)
}

/// `Album.wav` sorts first and folds the same, so only the spelling as written picks the other.
#[test]
fn a_name_resolves_as_written_first() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["Album.wav", "album.wav"])?;

    assert_eq!(resolved("album.wav", tmp.path(), "", &audio), Some(PathBuf::from("album.wav")));
    Ok(())
}

/// The `.AAC` beside it sorts first under the same stem, so only the name itself picks the FLAC.
#[test]
fn a_name_differing_in_case_resolves_to_the_file() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["Album.AAC", "Album.FLAC"])?;

    assert_eq!(resolved("album.flac", tmp.path(), "", &audio), Some(PathBuf::from("Album.FLAC")));
    Ok(())
}

/// A sheet written on macOS spells `é` decomposed where Linux keeps the composed form, or the
/// other way round.
#[test]
fn a_name_differing_in_normalization_or_accents_resolves_to_the_file() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["Cafe\u{301} del Mar.aac", "Cafe\u{301} del Mar.flac"])?;

    let rows = ["Caf\u{E9} del Mar.flac", "Cafe del Mar.flac"];
    for name in rows {
        let found = resolved(name, tmp.path(), "", &audio);
        assert_eq!(found, Some(PathBuf::from("Cafe\u{301} del Mar.flac")), "{name}");
    }
    Ok(())
}

#[test]
fn a_name_resolves_to_its_stem_under_another_audio_extension() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["album.flac"])?;

    assert_eq!(resolved("album.wav", tmp.path(), "", &audio), Some(PathBuf::from("album.flac")));
    Ok(())
}

/// `Album.flac` sorts first and shares the stem, so only the folded name picks the other.
#[test]
fn the_folded_name_wins_over_the_stem() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["Album.flac", "album.WAV"])?;

    assert_eq!(resolved("album.wav", tmp.path(), "", &audio), Some(PathBuf::from("album.WAV")));
    Ok(())
}

#[test]
fn a_stem_matching_two_files_resolves_to_the_lowest_path() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["album.mp3", "album.flac"])?;

    assert_eq!(resolved("album.wav", tmp.path(), "", &audio), Some(PathBuf::from("album.flac")));
    Ok(())
}

#[test]
fn a_relative_name_resolves_from_the_sheet_s_folder() -> Result<(), AppError> {
    let nested = Path::new("CD1").join("album.flac");
    let (tmp, audio) = walked(&[&nested.to_string_lossy(), "up.flac"])?;
    let rows = [
        ("CD1\\album.flac", "", Path::new("CD1").join("album.flac")),
        ("CD1/album.flac", "", Path::new("CD1").join("album.flac")),
        ("./CD1/album.flac", "", Path::new("CD1").join("album.flac")),
        ("..\\up.flac", "CD1", PathBuf::from("up.flac")),
    ];
    for (name, sheet_dir, expected) in rows {
        assert_eq!(resolved(name, tmp.path(), sheet_dir, &audio), Some(expected), "{name}");
    }
    Ok(())
}

#[test]
fn an_absolute_name_keeps_only_its_file_name() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["album.flac"])?;

    for name in ["C:\\Rips\\album.flac", "\\\\server\\rips\\album.flac", "/home/ripper/album.flac"]
    {
        let found = resolved(name, tmp.path(), "", &audio);
        assert_eq!(found, Some(PathBuf::from("album.flac")), "{name}");
    }
    Ok(())
}

#[test]
fn a_name_matching_nothing_walked_resolves_to_nothing() -> Result<(), AppError> {
    let (tmp, audio) = walked(&["album.flac"])?;

    for name in ["other.flac", "", ".", "CD2\\album.flac"] {
        assert_eq!(resolved(name, tmp.path(), "", &audio), None, "{name:?}");
    }
    Ok(())
}
