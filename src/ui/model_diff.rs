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
//! positionally, rewrite every row via `set_row_data`; otherwise fall
//! back to `set_vec`.
//!
//! Row types here don't implement `PartialEq` (`slint::Image` doesn't),
//! so we rewrite every aligned row rather than diffing per-field — still
//! strictly cheaper than a reset, and it ensures field updates always
//! propagate.

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
    T: Clone + 'static,
    F: Fn(&T) -> i32,
{
    let cur_count = vec_model.row_count();
    if cur_count == new_rows.len() {
        let mut all_match = true;
        for (i, new_r) in new_rows.iter().enumerate() {
            match vec_model.row_data(i) {
                Some(cur) if id_of(&cur) == id_of(new_r) => {}
                _ => {
                    all_match = false;
                    break;
                }
            }
        }
        if all_match {
            for (i, new_r) in new_rows.into_iter().enumerate() {
                vec_model.set_row_data(i, new_r);
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
