//! Wrapping a strip of chips into rows.
//!
//! Slint 1.16 has no `Flow` and can't build a nested array, so anything that wraps splits
//! here and hands the view two real arrays to walk. Two splits, and which one a surface
//! takes follows where its chips are measured: [`chunk_indices`] wraps by *index*, for a
//! host that measures real chips with a hidden ruler and can size every row off the widest;
//! [`chunk_chips_to_rows`] wraps by *width*, the chip texts being built here.
//!
//! The width form's consumers differ in one thing, and that is the whole of `max_rows`: Now
//! Playing has the column height to grow downward, where a hero band is sized by its artwork
//! tile and wraps only as far as the slack above its action pill.

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;

use melodia_ui::{AppWindow, Wrap};

/// Gap between chips within a row — `Theme.pad-sm`, restated because the wrap
/// has to know it and Slint tokens don't cross the boundary. The gap *between*
/// rows is `pad-xs` and is Slint's alone; nothing here measures vertically.
const SPACING: f32 = 8.0;

/// `MetaChip`'s fixed width: its `pad-md` left and right, with no close affordance.
const META_CHIP_CHROME: f32 = 24.0;

/// Estimated rendered chip width — Vazirmatn at `font-size-sm`, plus whatever of the chip
/// isn't glyphs (`chrome`, which differs per chip shape).
///
/// **The two error directions are not symmetric**, because this packs a row as full as the
/// estimate allows — unlike `ChipGroup`, which sizes every row off its widest chip and so
/// is never full. Over-shoot only wraps early; under-shoot compresses, and on `MetaChip` it
/// compresses the whole *row* rather than the chip that didn't fit, its label carrying
/// `overflow: elide` and so lowering each chip's layout *minimum* to one `…`. Hence a
/// generous `CHAR_W`, a little over half an em where Vazirmatn's digits sit near 0.55.
/// Both spacings are exact; only the glyph term estimates.
fn estimated_chip_width(text: &str, chrome: f32) -> f32 {
    const CHAR_W: f32 = 6.5;
    // Saturating to `u16` is ample headroom for a chip text, and `f32::from(u16)` avoids
    // the `cast_precision_loss` a direct `as f32` would trip.
    let chars = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    f32::from(chars) * CHAR_W + chrome
}

/// Greedily pack items into rows fitting `avail_width`, always at least one per row so an
/// oversized single item gets its own rather than none.
///
/// `max_rows` caps the result and **drops** the overflow; `None` wraps freely. Dropping is
/// what a fixed-height band wants: an item that can't fit is less important than the ones
/// before it, the builders ordering them that way, and growing the band under a resize
/// drag reads as the layout thrashing.
fn pack_rows<T: Clone>(
    items: &[T],
    avail_width: f32,
    max_rows: Option<usize>,
    width_of: impl Fn(&T) -> f32,
) -> Vec<Vec<T>> {
    if items.is_empty() || max_rows == Some(0) {
        return Vec::new();
    }
    // `<= 0` means no layout pass yet, so collapse to one row; a real width lands on the
    // frame after and re-packs.
    if avail_width <= 0.0 {
        return vec![items.to_vec()];
    }

    let mut rows: Vec<Vec<T>> = Vec::with_capacity(2);
    let mut current: Vec<T> = Vec::with_capacity(items.len());
    let mut current_w = 0.0_f32;

    for item in items {
        let w = width_of(item);
        let candidate = if current.is_empty() { w } else { current_w + SPACING + w };
        if !current.is_empty() && candidate > avail_width {
            if max_rows == Some(rows.len() + 1) {
                // The row being closed is the last one allowed, so the rest is overflow.
                return finish(rows, current);
            }
            rows.push(std::mem::take(&mut current));
            current.push(item.clone());
            current_w = w;
        } else {
            current.push(item.clone());
            current_w = candidate;
        }
    }
    finish(rows, current)
}

fn finish<T>(mut rows: Vec<Vec<T>>, current: Vec<T>) -> Vec<Vec<T>> {
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

/// [`pack_rows`] over `MetaChip`s, which is what the two `MetaChipStrip` hosts want.
pub fn chunk_chips_to_rows(
    chips: &[SharedString],
    avail_width: f32,
    max_rows: Option<usize>,
) -> Vec<Vec<SharedString>> {
    pack_rows(chips, avail_width, max_rows, |chip| estimated_chip_width(chip, META_CHIP_CHROME))
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

/// Split `0..count` into rows of at most `per_row`. Indices rather than the items themselves,
/// so a wrapped item still knows which option it is.
///
/// `per_row` is floored at 1: it comes from a measured width, which is zero for the frame
/// before the first layout reports one.
fn chunk_indices(count: i32, per_row: i32) -> Vec<Vec<i32>> {
    let count = count.max(0);
    let per_row = per_row.max(1);

    let row_count =
        usize::try_from(count).unwrap_or(0).div_ceil(usize::try_from(per_row).unwrap_or(1));
    let mut rows = Vec::with_capacity(row_count);
    let mut start = 0;
    while start < count {
        let end = start.saturating_add(per_row).min(count);
        rows.push((start..end).collect());
        start = end;
    }
    rows
}

/// Wire the two row splits. Call once during startup.
pub fn install(ui: &AppWindow) {
    let wrap = ui.global::<Wrap>();

    wrap.on_chunk_indices(|count, per_row| {
        let rows: Vec<ModelRc<i32>> = chunk_indices(count, per_row)
            .into_iter()
            .map(|row| ModelRc::from(Rc::new(VecModel::from(row))))
            .collect();
        ModelRc::from(Rc::new(VecModel::from(rows)))
    });

    // Wraps freely: the one caller is a page that scrolls, so there is no band height to
    // drop a row against.
    wrap.on_pack_labels(|labels, avail_width, chrome| {
        let labels: Vec<SharedString> = labels.iter().collect();
        let rows = pack_rows(&labels, avail_width, None, |l: &SharedString| {
            estimated_chip_width(l, chrome)
        });
        rows_to_model(rows)
    });
}

#[cfg(test)]
#[path = "tests/chips_tests.rs"]
mod tests;
