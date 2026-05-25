//! Technical-metadata chip row formatter + display helpers.

use crate::entities::track::TrackMeta;
use crate::TrackMetaRow;
use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// Format the `TrackMeta` projection into the pre-formatted display
/// strings the `TrackMetaRow` chip row reads. `""` for any absent field —
/// the view gates each chip on `field != ""`.
pub(super) fn to_slint_track_meta(t: &TrackMeta) -> TrackMetaRow {
    TrackMetaRow {
        track_id: i32::try_from(t.id).unwrap_or(i32::MAX),
        codec: t
            .codec
            .as_deref()
            .map(str::to_uppercase)
            .unwrap_or_default()
            .into(),
        bitrate: t
            .bitrate
            .map(|b| format!("{b} kbps"))
            .unwrap_or_default()
            .into(),
        sample_rate: t
            .sample_rate
            .map(format_sample_rate)
            .unwrap_or_default()
            .into(),
        bit_depth: t
            .bit_depth
            .map(|d| format!("{d}-bit"))
            .unwrap_or_default()
            .into(),
        channels: t.channels.map(format_channels).unwrap_or_default().into(),
        year: t
            .year
            .filter(|y| *y > 0)
            .map(|y| y.to_string())
            .unwrap_or_default()
            .into(),
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

/// Estimated rendered chip width — Vazirmatn at `font-size-sm` with the
/// `MetaChip`'s `pad-md` left+right padding. Approximate but stable;
/// `HorizontalLayout`'s `alignment: center` absorbs minor over/under shoot
/// and N-row wrap is forgiving in the wrap direction.
fn estimated_chip_width(text: &str) -> f32 {
    const CHAR_W: f32 = 6.5;
    const PAD: f32 = 24.0;
    // Chip texts are short (max a few dozen chars); saturating to `u16` is
    // ample headroom and `f32::from(u16)` avoids the `cast_precision_loss`
    // lint a direct `usize as f32` would trip.
    let chars = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    f32::from(chars) * CHAR_W + PAD
}

/// Chunk visible chips into rows so each row's total width (chip widths +
/// `pad-sm` spacing between chips) fits in `avail_width`. Always emits at
/// least one chip per row (an oversized single chip gets its own row).
pub(super) fn chunk_chips_to_rows(
    chips: &[SharedString],
    avail_width: f32,
) -> Vec<Vec<SharedString>> {
    const SPACING: f32 = 8.0;

    if chips.is_empty() {
        return Vec::new();
    }
    // `<= 0` means we haven't been laid out yet — collapse to one row; the
    // mount Timer in the view fires a real width immediately after.
    if avail_width <= 0.0 {
        return vec![chips.to_vec()];
    }

    let mut rows: Vec<Vec<SharedString>> = Vec::with_capacity(2);
    let mut current: Vec<SharedString> = Vec::with_capacity(chips.len());
    let mut current_w = 0.0_f32;

    for chip in chips {
        let cw = estimated_chip_width(chip);
        let candidate = if current.is_empty() {
            cw
        } else {
            current_w + SPACING + cw
        };
        if !current.is_empty() && candidate > avail_width {
            rows.push(std::mem::take(&mut current));
            current.push(chip.clone());
            current_w = cw;
        } else {
            current.push(chip.clone());
            current_w = candidate;
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

/// `Vec<Vec<SharedString>>` → `ModelRc<ModelRc<SharedString>>` suitable for
/// `Player::set_chip_rows`.
pub(super) fn rows_to_model(
    rows: Vec<Vec<SharedString>>,
) -> ModelRc<ModelRc<SharedString>> {
    let outer: Vec<ModelRc<SharedString>> = rows
        .into_iter()
        .map(|row| ModelRc::from(Rc::new(VecModel::from(row))))
        .collect();
    ModelRc::from(Rc::new(VecModel::from(outer)))
}
