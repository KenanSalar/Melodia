//! Technical-metadata chip row formatter + display helpers.

use crate::ui::util::{format_channels, format_sample_rate, opt_shared};
use melodia_core::entities::track::TrackMeta;
use melodia_ui::TrackMetaRow;
use slint::SharedString;

/// Formats `t` into the strings the Now Playing chips are built from, `""` where a
/// field is absent so [`visible_chip_texts`] leaves it out.
pub(super) fn to_slint_track_meta(t: &TrackMeta) -> TrackMetaRow {
    TrackMetaRow {
        track_id: i32::try_from(t.id).unwrap_or(i32::MAX),
        codec: opt_shared(t.codec.as_deref().map(str::to_uppercase)),
        bitrate: opt_shared(t.bitrate.map(|b| format!("{b} kbps"))),
        sample_rate: opt_shared(t.sample_rate.map(format_sample_rate)),
        bit_depth: opt_shared(t.bit_depth.map(|d| format!("{d}-bit"))),
        channels: opt_shared(t.channels.map(format_channels)),
        year: opt_shared(t.year.filter(|y| *y > 0).map(|y| y.to_string())),
        genre: opt_shared(t.genre.as_deref()),
    }
}

/// The non-empty fields of `m` as the Now Playing chips, in reading order.
pub(super) fn visible_chip_texts(m: &TrackMetaRow) -> Vec<SharedString> {
    let fields =
        [&m.codec, &m.bitrate, &m.sample_rate, &m.bit_depth, &m.channels, &m.year, &m.genre];
    let mut out = Vec::with_capacity(fields.len());
    for s in fields {
        if !s.is_empty() {
            out.push(s.clone());
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/metadata_tests.rs"]
mod tests;
