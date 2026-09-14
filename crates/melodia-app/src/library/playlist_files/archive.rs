//! The zip a multi-playlist export is saved as, and read back from.
//!
//! Only the container lives here: each entry's text is [`super::m3u`]'s, and which playlist
//! becomes which entry is the caller's. Nothing is ever extracted to disk, so an entry's name
//! matters only as the name its playlist falls back to and the folder its relative paths
//! resolve against.

use std::io::{Cursor, Read, Seek, Write};

use chrono::NaiveDateTime;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::{PLAYLIST_EXTENSIONS, has_extension};
use melodia_core::error::{AppError, describe};

/// How much playlist text one archive may expand to before it is refused whole.
///
/// A plain playlist file is bounded by its own size on disk and an archive is not, deflate being
/// built to expand. The budget counts bytes actually read rather than the sizes the archive
/// declares, which it is free to lie about. Set far past any real library, since landing on it
/// refuses the whole import.
pub const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;

/// One playlist file inside an archive.
pub struct Entry {
    /// Its path inside the archive, which is where it lands once extracted.
    pub name: String,
    pub text: String,
}

/// The playlist files an archive held.
#[derive(Default)]
pub struct Contents {
    /// In archive order.
    pub entries: Vec<Entry>,
    /// Playlist entries that couldn't be read, skipped so the rest still import.
    pub unreadable: u32,
}

/// Zips `entries` into an archive held in memory, each dated `modified`.
///
/// A date the format can't hold (before 1980 or past 2107) takes the format's own epoch rather
/// than failing an export over a clock.
pub fn write(entries: &[Entry], modified: NaiveDateTime) -> Result<Vec<u8>, AppError> {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::try_from(modified).unwrap_or_default());

    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    for entry in entries {
        archive.start_file(entry.name.as_str(), options).map_err(AppError::io_source)?;
        archive.write_all(entry.text.as_bytes())?;
    }
    Ok(archive.finish().map_err(AppError::io_source)?.into_inner())
}

/// Reads every `.m3u8` and `.m3u` entry out of an archive. Anything else in it is ignored.
///
/// # Errors
///
/// When the reader isn't a zip, or its playlists expand past [`MAX_TEXT_BYTES`]. A single entry
/// that won't read is counted in [`Contents::unreadable`] instead.
pub fn read<R: Read + Seek>(reader: R) -> Result<Contents, AppError> {
    let mut archive = ZipArchive::new(reader).map_err(AppError::io_source)?;
    let mut contents = Contents::default();
    let mut remaining = MAX_TEXT_BYTES;

    for index in 0..archive.len() {
        let is_playlist = archive
            .name_for_index(index)
            .is_some_and(|name| has_extension(name.as_ref(), &PLAYLIST_EXTENSIONS));
        if !is_playlist {
            continue;
        }

        let (name, bytes) = match read_entry(&mut archive, index, remaining) {
            Ok(Some(entry)) => entry,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("playlist archive: entry {index} won't read: {}", describe(&e));
                contents.unreadable = contents.unreadable.saturating_add(1);
                continue;
            }
        };

        let read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if read > remaining {
            return Err(AppError::Validation(format!(
                "Playlist archive expands past {MAX_TEXT_BYTES} bytes"
            )));
        }
        remaining -= read;

        match String::from_utf8(bytes) {
            Ok(text) => contents.entries.push(Entry { name, text }),
            Err(e) => {
                log::warn!("playlist archive: {name} is not UTF-8: {e}");
                contents.unreadable = contents.unreadable.saturating_add(1);
            }
        }
    }
    Ok(contents)
}

/// Reads one entry through one byte past `limit`, so the caller can tell an entry that stops on
/// the budget from one that runs over it.
///
/// `None` for a directory, and for a name that climbs out of the archive or is absolute: that
/// name would otherwise pick the folder its playlist's relative paths resolve against.
fn read_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    index: usize,
    limit: u64,
) -> Result<Option<(String, Vec<u8>)>, AppError> {
    let file = archive.by_index(index).map_err(AppError::io_source)?;
    if file.is_dir() {
        return Ok(None);
    }
    let Some(name) = file.enclosed_name() else {
        return Ok(None);
    };

    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok(Some((name.to_string_lossy().into_owned(), bytes)))
}
