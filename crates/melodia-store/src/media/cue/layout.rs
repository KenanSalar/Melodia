//! Where each of a sheet's tracks sits in the files it names.
//!
//! A track runs from its `INDEX 01` to the next track's, so the gap behind `INDEX 00` plays at the
//! end of the track before it, where the disc puts it played straight through. It belongs to the
//! file its `INDEX 01` sits under, and the last track of a file plays to that file's end. `PREGAP`
//! and `POSTGAP` are silence a burner generates, with nothing in the file to move a boundary.

use std::collections::BTreeMap;
use std::path::Path;

use super::parse::Sheet;
use melodia_core::entities::cue::{CueSpan, CueTrack, FRAMES_PER_SECOND};

/// The fewest cue frames of audio ahead of track 1 that make a hidden track. Less is lead-in, which
/// a disc's first pregap often holds a few frames of.
const HIDDEN_TRACK_FLOOR: u32 = FRAMES_PER_SECOND;

/// The fewest rows a sheet carves out of a file to split it. One track from the top is a per-track
/// rip, whose file keeps its own row and tags.
pub(crate) const MIN_CLAIMED_ROWS: usize = 2;

/// The rows a sheet carves out of one of its files, in track order.
pub(crate) struct FileLayout {
    /// The file, as a position in [`Sheet::files`].
    pub(crate) file: usize,
    pub(crate) rows: Vec<PlacedTrack>,
}

impl FileLayout {
    /// Whether the sheet splits the file.
    pub(crate) fn claims(&self) -> bool {
        self.rows.len() >= MIN_CLAIMED_ROWS
    }

    /// The sheet's numbered tracks on this file, the count a row's `track_total` states.
    pub(crate) fn track_total(&self) -> usize {
        self.rows.iter().filter(|row| row.track != CueTrack::HIDDEN).count()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PlacedTrack {
    pub(crate) track: CueTrack,
    pub(crate) span: CueSpan,
    /// The sheet track the row takes its fields from, a position in [`Sheet::tracks`]: `None` for
    /// the hidden track, which no sheet names.
    pub(crate) source: Option<usize>,
}

/// Lays `sheet` out over its files, logging against `origin` each track it drops. A track that isn't
/// audio takes no row and keeps its number, and still ends the track before it.
pub(crate) fn lay_out(sheet: &Sheet, origin: &Path) -> Vec<FileLayout> {
    let starts: Vec<_> = sheet
        .tracks
        .iter()
        .enumerate()
        .filter_map(|(source, track)| track.index(1).map(|start| (source, start)))
        .collect();

    let mut by_file: BTreeMap<usize, Vec<PlacedTrack>> = BTreeMap::new();
    for (position, &(source, start)) in starts.iter().enumerate() {
        let track = &sheet.tracks[source];
        if !track.is_audio {
            continue;
        }
        let end = starts
            .get(position + 1)
            .map(|&(_, next)| next)
            .filter(|next| next.file == start.file)
            .map(|next| next.at);
        let number = track.number.number();
        let Ok(span) = CueSpan::new(start.at, end) else {
            log::warn!("Dropped track {number} of {}: it ends before it starts", origin.display());
            continue;
        };
        let rows = by_file.entry(start.file).or_default();
        if rows.iter().any(|row| row.track == track.number) {
            log::warn!("Dropped a second track {number} from {}", origin.display());
            continue;
        }
        rows.push(PlacedTrack { track: track.number, span, source: Some(source) });
    }

    if let Some((file, hidden)) = hidden_track(sheet) {
        by_file.entry(file).or_default().push(hidden);
    }
    by_file
        .into_iter()
        .map(|(file, mut rows)| {
            rows.sort_by_key(|row| row.track);
            FileLayout { file, rows }
        })
        .collect()
}

/// The audio between track 1's `INDEX 00` and its `INDEX 01`, where a disc hides a track ahead of
/// the first, with the file it sits in.
fn hidden_track(sheet: &Sheet) -> Option<(usize, PlacedTrack)> {
    let first = sheet.tracks.iter().find(|track| track.number.number() == 1 && track.is_audio)?;
    let (lead_in, start) = (first.index(0)?, first.index(1)?);
    let length = i64::from(start.at) - i64::from(lead_in.at);
    if lead_in.file != start.file || length < i64::from(HIDDEN_TRACK_FLOOR) {
        return None;
    }
    let span = CueSpan::new(lead_in.at, Some(start.at)).ok()?;
    Some((start.file, PlacedTrack { track: CueTrack::HIDDEN, span, source: None }))
}

#[cfg(test)]
#[path = "tests/layout_tests.rs"]
mod tests;
