//! One sheet track's tags for Edit Tags, where its row holds none: a track filed outside the
//! library has no join rows and no album, and its image's tags state the whole disc.

use std::path::Path;

use super::merge::track_tags;
use super::parse::parse;
use super::text::read_sheet;
use crate::media::ingest::tag_writer::read_file_tags;
use melodia_core::entities::cue::CueTrack;
use melodia_core::entities::tags::FileTags;
use melodia_core::error::AppError;

/// Track `track` of the sheet at `sheet`, read over the tags of its image at `image` as the scan
/// merges them. Blocking.
pub fn read_track_tags(sheet: &Path, image: &Path, track: CueTrack) -> Result<FileTags, AppError> {
    let stated = parse(&read_sheet(sheet)?, sheet);
    Ok(track_tags(&stated, track, read_file_tags(image)?))
}
