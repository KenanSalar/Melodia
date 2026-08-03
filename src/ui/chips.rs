//! Wrapping a strip of metadata chips into rows.
//!
//! Slint 1.16 has no `Flow` and can't build a nested array, so anything that
//! wraps does the split here and hands the view two real arrays to walk — the
//! same shape as [`crate::ui::settings_page::chunk_indices`], which wraps by
//! *index* because its chips are measured Slint-side. These are wrapped by
//! *width*, because the chip texts are built in Rust and never leave it.
//!
//! Two consumers, and they differ in exactly one thing. The Now Playing view
//! has the column height to grow downward, so it wraps freely; a hero band is
//! sized by its artwork tile, so it wraps only as far as the slack above its
//! action pill and drops the rest. That is the whole of `max_rows`.

use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// Gap between chips, and between rows — `Theme.pad-sm`, restated because the
/// wrap has to know it and Slint tokens don't cross the boundary.
const SPACING: f32 = 8.0;

/// Estimated rendered chip width — Vazirmatn at `font-size-sm` with
/// `MetaChip`'s `pad-md` left+right padding.
///
/// **The two error directions are not symmetric**, because this packs a row as
/// full as the estimate allows — unlike `ChipGroup`, which sizes every row off
/// its widest chip and so is never full at all. Over-shoot only wraps early;
/// under-shoot overflows, and nothing downstream absorbs it, since a no-wrap
/// `Text` reports its full string as its layout *minimum* and the row is
/// therefore incompressible. So `CHAR_W` leans generous: 6.5 px against a 12 px
/// `font-size-sm` is ≈0.54 em, where Vazirmatn's digits sit near 0.55 and its
/// lowercase near 0.5, and chip texts are counts, years and short words. Both
/// spacings are exact (`pad-sm`, `2 × pad-md`) — only the glyph term estimates.
fn estimated_chip_width(text: &str) -> f32 {
    const CHAR_W: f32 = 6.5;
    const PAD: f32 = 24.0;
    // Chip texts are short (max a few dozen chars); saturating to `u16` is
    // ample headroom and `f32::from(u16)` avoids the `cast_precision_loss`
    // lint a direct `usize as f32` would trip.
    let chars = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    f32::from(chars) * CHAR_W + PAD
}

/// Chunk chips into rows so each row's total width (chip widths + the spacing
/// between them) fits in `avail_width`. Always emits at least one chip per row,
/// so an oversized single chip gets its own row rather than none.
///
/// `max_rows` caps the result and **drops** the overflow — `None` wraps freely.
/// Dropping past a cap is what a fixed-height band wants: a chip that can't fit
/// is less important than the ones before it (the builders order them that
/// way), and growing the band under a resize drag reads as the layout
/// thrashing.
pub fn chunk_chips_to_rows(
    chips: &[SharedString],
    avail_width: f32,
    max_rows: Option<usize>,
) -> Vec<Vec<SharedString>> {
    if chips.is_empty() || max_rows == Some(0) {
        return Vec::new();
    }
    // `<= 0` means we haven't been laid out yet — collapse to one row; the
    // strip's mount Timer fires a real width immediately after.
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
                // The row we'd be closing is the last one allowed, so
                // everything from here on is overflow.
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

fn finish(
    mut rows: Vec<Vec<SharedString>>,
    current: Vec<SharedString>,
) -> Vec<Vec<SharedString>> {
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

/// `Vec<Vec<SharedString>>` → the `[[string]]` model a `MetaChipStrip` reads.
pub fn rows_to_model(rows: Vec<Vec<SharedString>>) -> ModelRc<ModelRc<SharedString>> {
    let outer: Vec<ModelRc<SharedString>> = rows
        .into_iter()
        .map(|row| ModelRc::from(Rc::new(VecModel::from(row))))
        .collect();
    ModelRc::from(Rc::new(VecModel::from(outer)))
}

#[cfg(test)]
#[path = "tests/chips_tests.rs"]
mod tests;
