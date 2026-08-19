//! Wrapping a strip of metadata chips into rows.
//!
//! Slint 1.16 has no `Flow` and can't build a nested array, so anything that wraps splits
//! here and hands the view two real arrays to walk — the same shape as
//! `settings::settings_page::chunk_indices`, which wraps by *index* because its chips are
//! measured Slint-side. These wrap by *width*, the chip texts being built in Rust.
//!
//! The two consumers differ in one thing, and that is the whole of `max_rows`: Now Playing
//! has the column height to grow downward, where a hero band is sized by its artwork tile
//! and wraps only as far as the slack above its action pill.

use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// Gap between chips within a row — `Theme.pad-sm`, restated because the wrap
/// has to know it and Slint tokens don't cross the boundary. The gap *between*
/// rows is `pad-xs` and is Slint's alone; nothing here measures vertically.
const SPACING: f32 = 8.0;

/// Estimated rendered chip width — Vazirmatn at `font-size-sm` with
/// `MetaChip`'s `pad-md` left+right padding.
///
/// **The two error directions are not symmetric**, because this packs a row as full as the
/// estimate allows — unlike `ChipGroup`, which sizes every row off its widest chip and so
/// is never full. Over-shoot only wraps early; under-shoot truncates, and it truncates the
/// whole *row* rather than the chip that didn't fit, `MetaChip`'s label carrying
/// `overflow: elide` and so lowering each chip's layout *minimum* to one `…`. Hence a
/// generous `CHAR_W`, a little over half an em where Vazirmatn's digits sit near 0.55.
/// Both spacings are exact; only the glyph term estimates.
fn estimated_chip_width(text: &str) -> f32 {
    const CHAR_W: f32 = 6.5;
    const PAD: f32 = 24.0;
    // Saturating to `u16` is ample headroom for a chip text, and `f32::from(u16)` avoids
    // the `cast_precision_loss` a direct `as f32` would trip.
    let chars = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    f32::from(chars) * CHAR_W + PAD
}

/// Chunk chips into rows fitting `avail_width`, always at least one per row so an
/// oversized single chip gets its own rather than none.
///
/// `max_rows` caps the result and **drops** the overflow; `None` wraps freely. Dropping is
/// what a fixed-height band wants: a chip that can't fit is less important than the ones
/// before it, the builders ordering them that way, and growing the band under a resize
/// drag reads as the layout thrashing.
pub fn chunk_chips_to_rows(
    chips: &[SharedString],
    avail_width: f32,
    max_rows: Option<usize>,
) -> Vec<Vec<SharedString>> {
    if chips.is_empty() || max_rows == Some(0) {
        return Vec::new();
    }
    // `<= 0` means no layout pass yet, so collapse to one row; the strip's mount Timer
    // fires a real width immediately after.
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
            if max_rows == Some(rows.len() + 1) {
                // The row being closed is the last one allowed, so the rest is overflow.
                return finish(rows, current);
            }
            rows.push(std::mem::take(&mut current));
            current.push(chip.clone());
            current_w = cw;
        } else {
            current.push(chip.clone());
            current_w = candidate;
        }
    }
    finish(rows, current)
}

fn finish(mut rows: Vec<Vec<SharedString>>, current: Vec<SharedString>) -> Vec<Vec<SharedString>> {
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

/// A split's row lengths — enough to tell two splits of the *same* chips apart.
///
/// Both strips re-chunk on `MetaChipStrip`'s `measured`, which fires per pointer motion of
/// a resize drag, and handing the result to Slint is a `set_rows` — a model *reset*,
/// rebuilding the whole chip repeater even for a byte-identical split, where a drag
/// crosses a wrap threshold once or twice.
///
/// Shape alone, not the chips: comparing those is the caller's half and each knows its own
/// answer for free.
pub fn split_shape(rows: &[Vec<SharedString>]) -> Vec<usize> {
    rows.iter().map(Vec::len).collect()
}

/// `Vec<Vec<SharedString>>` → the `[[string]]` model a `MetaChipStrip` reads.
pub fn rows_to_model(rows: Vec<Vec<SharedString>>) -> ModelRc<ModelRc<SharedString>> {
    let outer: Vec<ModelRc<SharedString>> =
        rows.into_iter().map(|row| ModelRc::from(Rc::new(VecModel::from(row)))).collect();
    ModelRc::from(Rc::new(VecModel::from(outer)))
}

#[cfg(test)]
#[path = "tests/chips_tests.rs"]
mod tests;
