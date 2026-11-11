//! A sheet a FLAC carries inside itself: a `CUESHEET` comment holding a `.cue`'s text, or the
//! native CUESHEET block `metaflac --import-cuesheet-from` writes, which states times and ISRCs but
//! no text. Its tracks are carved where the image's rows are written, as an external sheet's are.
//!
//! The comment wins where it places a track, being the one a tagger writes titles into, and an
//! external `.cue` wins over both: the store writes an image a sheet claims through that sheet's
//! job, ahead of anything asking this module.

mod block;
mod chain;
mod overrides;

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::AudioFile;
use lofty::flac::FlacFile;
use lofty::ogg::tag::VorbisComments;

use super::layout::{self, FileLayout};
use super::merge::{self, CueRow};
use super::parse::{self, Files, Sheet};
use super::write::{self, SheetEdit, SheetRows};
use crate::media::ingest::metadata::{self, TagScope};
use crate::media::ingest::tag_writer::file_tags_of;
use melodia_core::entities::cue::{CueTrack, EmbeddedCue, EmbeddedSheet, EmbeddedSource};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::tags::{EmbeddedSheetTags, FileTags};
use melodia_core::error::{AppError, describe};
use melodia_core::utils::atomic_file;

pub use chain::carries_sheet;
pub(crate) use overrides::TrackField;

/// The comment key a sheet's text sits under.
const SHEET_KEY: &str = "CUESHEET";

/// The sheet the FLAC at `path` carries, its comments already read: the `CUESHEET` comment where it
/// places a track, else the native block, read off the file here. Blocking.
pub(crate) fn read(path: &Path, comments: Option<&VorbisComments>) -> EmbeddedCue {
    // A sheet copied in from a `.cue` saved with a BOM keeps it, which would hide its first line.
    let comment = comments
        .and_then(|comments| comments.get(SHEET_KEY))
        .map(|text| text.strip_prefix('\u{FEFF}').unwrap_or(text))
        .filter(|text| {
            let (stated, _) = parse::statements(text.lines(), Files::Inside);
            stated.places_a_track()
        });
    let source = match comment {
        Some(text) => EmbeddedSource::Comment(text.to_owned()),
        None => match chain::read_block(path) {
            Ok(Some(block)) => block,
            Ok(None) => return EmbeddedCue::Absent,
            Err(e) => {
                log::warn!("Unreadable CUESHEET block in {}: {}", path.display(), describe(&e));
                return EmbeddedCue::Unread;
            }
        },
    };
    let overrides = comments
        .iter()
        .flat_map(|comments| comments.items())
        .filter(|(key, _)| overrides::override_key(key).is_some())
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    EmbeddedCue::Present(EmbeddedSheet { source, overrides })
}

/// The rows the sheet inside the image at `path` carves out of it, whose own read is `disc`, or
/// `None` where it carries no sheet that splits it.
pub(crate) fn carve(path: &Path, disc: &ExtractedMetadata) -> Option<Vec<CueRow>> {
    let EmbeddedCue::Present(embedded) = &disc.embedded_cue else {
        return None;
    };
    let sheet = sheet_of(embedded, path)?;
    let layout = layout::lay_out(&sheet, path).into_iter().find(FileLayout::claims)?;
    merge::rows(&sheet, &layout, path, disc)
}

fn sheet_of(embedded: &EmbeddedSheet, origin: &Path) -> Option<Sheet> {
    let mut sheet = match &embedded.source {
        EmbeddedSource::Comment(text) => parse::parse(text, origin, Files::Inside),
        EmbeddedSource::Block { sample_rate, bytes } => block::sheet(*sample_rate, bytes)?,
    };
    overrides::apply(&mut sheet, &embedded.overrides);
    Some(sheet)
}

/// Reads the image at `image` and the sheet inside it for Edit Tags, with the tags of `tracks`
/// read over the image's as the scan merges them. Blocking.
pub fn read_embedded(image: &Path, tracks: &[CueTrack]) -> Result<EmbeddedSheetTags, AppError> {
    let (tagged, cue) = metadata::read_tags_and_cue(image, TagScope::TagsOnly)?;
    let tags = file_tags_of(&tagged);
    let (sheet, is_editable) = match &cue {
        EmbeddedCue::Present(embedded) => {
            let is_editable = matches!(embedded.source, EmbeddedSource::Comment(_));
            (sheet_of(embedded, image), is_editable)
        }
        EmbeddedCue::Absent | EmbeddedCue::Unread => (None, false),
    };
    let track_tags = |track: CueTrack| match &sheet {
        Some(sheet) => merge::track_tags(sheet, track, tags.clone()),
        None => FileTags { lyrics: None, ..tags.clone() },
    };
    let tracks = tracks.iter().map(|&track| (track, track_tags(track))).collect();
    Ok(EmbeddedSheetTags { is_editable, tracks })
}

/// The image at `image` with `edit` written into its `CUESHEET` comment for `rows`, or `None` where
/// the comment states all of it already. A track's override of a field the edit states goes, so
/// the sheet's line is what reads back. Blocking.
pub fn rewritten_embedded(
    image: &Path,
    edit: &SheetEdit,
    rows: &SheetRows,
) -> Result<Option<EmbeddedWrite>, AppError> {
    let mut flac = metadata::read_flac(image)?;
    let no_text =
        || AppError::metadata_msg(format!("{} holds no cue sheet text to write", image.display()));
    let comments = flac.vorbis_comments_mut().ok_or_else(no_text)?;
    let text = comments.get(SHEET_KEY).ok_or_else(no_text)?;
    let rewrite = write::rewritten_comment(text, image, edit, rows);

    let is_stale =
        |key: &str| overrides::override_key(key).is_some_and(|field| rewrite.states(field));
    let has_stale = comments.items().any(|(key, _)| is_stale(key));
    if has_stale {
        // Rebuilt rather than `remove`d: lofty's `remove` reorders the comments it keeps.
        for (key, value) in comments.take_items() {
            if !is_stale(&key) {
                comments.push(key, value);
            }
        }
    }
    let changed = rewrite.text.is_some() || has_stale;
    if let Some(text) = rewrite.text {
        comments.insert(SHEET_KEY.to_owned(), text);
    }
    Ok(changed.then_some(EmbeddedWrite(flac)))
}

/// An image with the sheet inside it rewritten, to save.
pub struct EmbeddedWrite(FlacFile);

impl EmbeddedWrite {
    /// Saves the image at `image` through a copy renamed over it, as a tag write does, so a deck
    /// playing it reads on from the file it opened. The whole `FlacFile` is saved, lofty dropping
    /// every picture a bare comment block doesn't hold. Blocking.
    pub fn save(self, image: &Path) -> Result<(), AppError> {
        atomic_file::rewrite_with_sync(image, move |copy| {
            self.0.save_to_path(copy, WriteOptions::default()).map_err(|e| {
                AppError::metadata(format!("Failed to write tags to {}", image.display()), e)
            })
        })
    }
}

#[cfg(test)]
#[path = "tests/embedded_tests.rs"]
mod tests;
