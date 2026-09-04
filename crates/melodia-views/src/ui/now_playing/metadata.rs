//! Technical-metadata chip row formatter + display helpers.

use melodia_core::entities::track::TrackMeta;
use melodia_ui::TrackMetaRow;
use slint::SharedString;

/// Format the `TrackMeta` projection into the pre-formatted display
/// strings the `TrackMetaRow` chip row reads. `""` for any absent field —
/// the view gates each chip on `field != ""`.
pub(super) fn to_slint_track_meta(t: &TrackMeta) -> TrackMetaRow {
    TrackMetaRow {
        track_id: i32::try_from(t.id).unwrap_or(i32::MAX),
        codec: t.codec.as_deref().map(str::to_uppercase).unwrap_or_default().into(),
        bitrate: t.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default().into(),
        sample_rate: t.sample_rate.map(format_sample_rate).unwrap_or_default().into(),
        bit_depth: t.bit_depth.map(|d| format!("{d}-bit")).unwrap_or_default().into(),
        channels: t.channels.map(format_channels).unwrap_or_default().into(),
        year: t.year.filter(|y| *y > 0).map(|y| y.to_string()).unwrap_or_default().into(),
        genre: t.genre.as_deref().unwrap_or("").into(),
    }
}

/// Hz → "44.1 kHz" / "48 kHz" (drops a trailing ".0").
pub(super) fn format_sample_rate(hz: i32) -> String {
    let khz = f64::from(hz) / 1000.0;
    if khz.fract().abs() < f64::EPSILON {
        format!("{khz:.0} kHz")
    } else {
        format!("{khz:.1} kHz")
    }
}

/// Channel count → "Mono" / "Stereo" / "N channels". Technical terms left
/// untranslated for v1 (built in Rust; Slint's `@tr` only covers literals).
pub(super) fn format_channels(n: i32) -> String {
    match n {
        1 => "Mono".to_string(),
        2 => "Stereo".to_string(),
        n => format!("{n} channels"),
    }
}

/// Walks `TrackMetaRow` fields in the same order the declarative chip block
/// in `now-playing-view.slint` declared them and returns the non-empty
/// texts. Used both to seed the chip shadow on track-meta change and to
/// re-chunk on width changes without re-reading the global.
pub(super) fn visible_chip_texts(m: &TrackMetaRow) -> Vec<SharedString> {
    let fields = [
        &m.codec,
        &m.bitrate,
        &m.sample_rate,
        &m.bit_depth,
        &m.channels,
        &m.year,
        &m.genre,
    ];
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
