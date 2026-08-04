//! Reusable `VecModel<T>` swap helper that prefers per-row updates over
//! full-model replacement.
//!
//! Slint's `VecModel::set_vec` fires a `reset()` notification that tears
//! down and re-instantiates every visible `ListView` delegate — costly
//! and visually flickery. `set_row_data` only fires `row_changed`, so the
//! `ListView` keeps its delegate cache.
//!
//! This helper runs the same fast-path used by `tracks::apply_visible`:
//! if the new vec is the same length as the model and ids align
//! positionally, write the rows that actually differ via `set_row_data`;
//! otherwise fall back to `set_vec`.
//!
//! Comparing is worth it because every generated row type is `PartialEq`
//! and cheap to compare: `slint::Image`'s equality bottoms out in a
//! pointer compare on the shared pixel buffer, not a pixel walk, so two
//! rows handed the same cached cover compare equal without touching the
//! data. A false "changed" verdict is therefore the only possible error
//! and it just writes the row, which is what the helper used to do
//! unconditionally. The win is in the common case: a track advance with
//! the whole library queued used to fire one `row_changed` per library
//! track for a row set in which nothing moved.

use slint::{Model, ModelRc, VecModel};

/// Apply `new_rows` to `vec_model`, preferring per-row `set_row_data` when
/// `new_rows` and `vec_model` describe the same row identities in the same
/// positions. Falls back to `set_vec` on any structural change (length
/// change, reorder, sort change).
///
/// `id_of` extracts the stable row identity used to decide whether the
/// fast path is safe.
pub fn apply_rows_keyed<T, F>(vec_model: &VecModel<T>, new_rows: Vec<T>, id_of: F)
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> i32,
{
    let cur_count = vec_model.row_count();
    if cur_count == new_rows.len() {
        // One pass decides both questions. The id check needs each current
        // row in hand anyway, so comparing it in full costs nothing extra
        // and tells the write loop below which rows to skip.
        let mut differs = Vec::with_capacity(cur_count);
        let mut all_match = true;
        for (i, new_r) in new_rows.iter().enumerate() {
            match vec_model.row_data(i) {
                Some(cur) if id_of(&cur) == id_of(new_r) => differs.push(cur != *new_r),
                _ => {
                    all_match = false;
                    break;
                }
            }
        }
        if all_match {
            for (i, new_r) in new_rows.into_iter().enumerate() {
                if differs[i] {
                    vec_model.set_row_data(i, new_r);
                }
            }
            return;
        }
    }
    vec_model.set_vec(new_rows);
}

/// Empty a `VecModel<T>`-backed model in place, logging a downcast miss under
/// `label`. Section-leave teardown uses this so the model's `SharedString`s /
/// row structs drop on the same UI tick as the Image-property release.
pub fn clear_vec_model<T: Clone + 'static>(model: &ModelRc<T>, label: &str) {
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<T>>() {
        vm.set_vec(Vec::new());
    } else {
        log::warn!("{label}: VecModel downcast failed");
    }
}

#[cfg(test)]
#[path = "tests/model_diff_tests.rs"]
mod tests;
