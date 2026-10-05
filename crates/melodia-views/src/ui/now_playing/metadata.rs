//! Technical-metadata chip row formatter + display helpers.

use crate::ui::util::{format_channels, format_sample_rate, opt_shared};
use melodia_core::entities::track::TrackMeta;
use melodia_ui::TrackMetaRow;
use slint::SharedString;

/// Format the `TrackMeta` projection into the pre-formatted display
/// strings the `TrackMetaRow` chip row reads. `""` for any absent field —
/// the view gates each chip on `field != ""`.
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

/// Walks `TrackMetaRow` fields in the same order the declarative chip block
/// in `now-playing-view.slint` declared them and returns the non-empty
/// texts. Used both to seed the chip shadow on track-meta change and to
/// re-chunk on width changes without re-reading the global.
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

// The wrap itself lives in `crate::ui::chips` — shared with the hero bands, so
// both strips break the same way and only the row cap differs.

#[cfg(test)]
#[path = "tests/metadata_tests.rs"]
mod tests;
