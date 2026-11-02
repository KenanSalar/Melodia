//! What the finder's rows show: a header from its group and its best copy, a copy from what the
//! fetch read of it.

use std::collections::HashMap;
use std::path::Path;

use slint::SharedString;

use super::picks::{DuplicateSet, HeaderFacts};
use crate::ui::paged_rows::PagedRow;
use crate::ui::util::{
    clamp_i64_to_i32, format_bit_depth, format_bitrate, format_codec, format_sample_rate,
    len_as_i32, opt_shared,
};
use melodia_core::entities::audio_quality::AudioFormat;
use melodia_core::entities::duplicates::{DuplicateCopy, MatchKind, ReviewReason};
use melodia_ui::{DuplicateMatch, DuplicateRow};

impl PagedRow for DuplicateRow {
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), is_header: id < 0, ..Self::default() }
    }

    fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
    }
}

/// The tracks a fetch reads for `asked`: each copy itself, and each header its best copy.
pub(super) fn track_ids_for(set: &DuplicateSet, asked: &[i64]) -> Vec<i64> {
    let mut ids: Vec<i64> = asked
        .iter()
        .filter_map(|&id| if id < 0 { set.header(id).map(|facts| facts.lead) } else { Some(id) })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The rows for `asked` out of what the fetch read. A copy gone since the find has no row, and
/// stays a placeholder.
pub(super) fn build(
    set: &DuplicateSet,
    asked: &[i64],
    copies: &[DuplicateCopy],
) -> Vec<(i64, DuplicateRow)> {
    let by_id: HashMap<i64, &DuplicateCopy> = copies.iter().map(|copy| (copy.id, copy)).collect();
    asked
        .iter()
        .filter_map(|&id| {
            let row = if id < 0 {
                let facts = set.header(id)?;
                header_row(id, &facts, by_id.get(&facts.lead)?)
            } else {
                copy_row(by_id.get(&id)?, set.copy_dismissed(id))
            };
            Some((id, row))
        })
        .collect()
}

fn header_row(id: i64, facts: &HeaderFacts, lead: &DuplicateCopy) -> DuplicateRow {
    DuplicateRow {
        id: clamp_i64_to_i32(id),
        is_header: true,
        dismissed: facts.dismissed,
        title: lead.title.as_str().into(),
        artist: opt_shared(lead.artist.as_deref()),
        match_kind: match_of(facts.kind),
        copy_count: len_as_i32(facts.copy_count),
        ..DuplicateRow::default()
    }
}

fn copy_row(copy: &DuplicateCopy, dismissed: bool) -> DuplicateRow {
    let (file_name, folder) = file_and_folder(&copy.file_path);
    DuplicateRow {
        id: clamp_i64_to_i32(copy.id),
        dismissed,
        file_name,
        format: format_label(&copy.format).into(),
        lossless: copy.format.is_lossless(),
        release: release(copy).into(),
        folder,
        added: day_of(&copy.date_added).into(),
        plays: clamp_i64_to_i32(copy.play_count),
        rating: copy.rating,
        favorite: copy.is_favorite,
        playlists: clamp_i64_to_i32(copy.playlist_count),
        ..DuplicateRow::default()
    }
}

fn match_of(kind: MatchKind) -> DuplicateMatch {
    match kind {
        MatchKind::Identical => DuplicateMatch::Identical,
        MatchKind::SameRelease => DuplicateMatch::SameRelease,
        MatchKind::Review(ReviewReason::NoArtist) => DuplicateMatch::NoArtist,
        MatchKind::Review(ReviewReason::OtherRecording) => DuplicateMatch::OtherRecording,
        MatchKind::Review(ReviewReason::OtherRelease) => DuplicateMatch::OtherRelease,
        MatchKind::Review(ReviewReason::OtherPosition) => DuplicateMatch::OtherPosition,
    }
}

/// "FLAC · 24-bit · 96 kHz" or "MPEG · 320 kbps": depth and rate are what a lossless encode
/// holds, bitrate what a lossy one kept.
fn format_label(format: &AudioFormat) -> String {
    let codec = format.codec.as_deref().map(format_codec);
    let detail = if format.is_lossless() {
        [format.bit_depth.map(format_bit_depth), format.sample_rate.map(format_sample_rate)]
    } else {
        [format.bitrate.map(format_bitrate), None]
    };
    std::iter::once(codec).chain(detail).flatten().collect::<Vec<_>>().join(" · ")
}

/// The album and year: what tells one release of a song from another, a remaster tagged like its
/// original differing only in the year.
fn release(copy: &DuplicateCopy) -> String {
    let year = copy.year.filter(|year| *year > 0).map(|year| year.to_string());
    [copy.album.clone(), year]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A file's name and the folder holding it, as a row shows them.
pub(super) fn file_and_folder(path: &str) -> (SharedString, SharedString) {
    let path = Path::new(path);
    let file_name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
    let folder = path.parent().map(|folder| folder.display().to_string()).unwrap_or_default();
    (file_name.as_ref().into(), folder.into())
}

/// The day of an RFC 3339 stamp, all it takes to tell an old copy from a recent one.
pub(super) fn day_of(stamp: &str) -> &str {
    stamp.split(['T', ' ']).next().unwrap_or_default()
}
