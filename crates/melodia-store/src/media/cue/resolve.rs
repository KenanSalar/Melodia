//! A sheet's `FILE` name to the walked audio file it means. Sheets move between machines and get
//! re-encoded beside their image, so the name as written is only the first try. The type keyword
//! after it is wrong often enough that it decides nothing.

use std::path::{Path, PathBuf};

use crate::media::ingest::scanner::WalkIndex;
use melodia_core::utils::fold::fold;

/// The audio file `name` means, from a sheet in `sheet_dir`: the path as written, then the same
/// name ignoring case, accents and normalization form, then the same stem under another audio
/// extension, as for `album.wav` re-encoded to `album.flac`. A tie goes to the lowest path.
pub(crate) fn resolve(name: &str, sheet_dir: &Path, audio: &WalkIndex) -> Option<PathBuf> {
    let written = written_path(name, sheet_dir)?;
    if audio.contains(&written) {
        return Some(written);
    }
    let dir = written.parent()?;
    let wanted_name = fold(written.file_name()?.to_str()?);
    let wanted_stem = fold(written.file_stem()?.to_str()?);
    let found = audio
        .in_dir(dir)
        .find(|path| {
            path.file_name().and_then(|n| n.to_str()).is_some_and(|n| fold(n) == wanted_name)
        })
        .or_else(|| {
            audio.in_dir(dir).find(|path| {
                path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| fold(s) == wanted_stem)
            })
        });
    found.map(Path::to_path_buf)
}

/// `name` as a path from `sheet_dir`, split on both separators since a sheet written on Windows
/// names `CD1\Album.wav`. An absolute path names a folder on the ripper's disk, so only its file
/// name is kept.
fn written_path(name: &str, sheet_dir: &Path) -> Option<PathBuf> {
    let mut components = name.split(['/', '\\']).filter(|part| !part.is_empty() && *part != ".");
    let names_another_disk = name.starts_with(['/', '\\']) || name.as_bytes().get(1) == Some(&b':');
    if names_another_disk {
        return components.next_back().map(|file_name| sheet_dir.join(file_name));
    }
    let mut path = sheet_dir.to_path_buf();
    for part in components {
        if part == ".." {
            path.pop();
        } else {
            path.push(part);
        }
    }
    (path != sheet_dir).then_some(path)
}

#[cfg(test)]
#[path = "tests/resolve_tests.rs"]
mod tests;
