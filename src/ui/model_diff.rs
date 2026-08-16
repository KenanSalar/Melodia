//! Reusable `VecModel<T>` swap helper preferring per-row updates over full-model
//! replacement.
//!
//! Slint's `VecModel::set_vec` fires a `reset()` that tears down and re-instantiates every
//! visible `ListView` delegate; `set_row_data` only fires `row_changed`, so the delegate
//! cache survives. So: same length and positionally-aligned ids → write the rows that
//! actually differ, otherwise fall back to `set_vec`.
//!
//! Comparing is worth it because every generated row type is `PartialEq` and cheap —
//! `slint::Image`'s equality bottoms out in a pointer compare on the shared pixel buffer,
//! so two rows handed the same cached cover compare equal without touching the data. A
//! false "changed" verdict is the only possible error and it just writes the row.

use slint::{Model, ModelRc, VecModel};

/// Apply `new_rows` to `vec_model`, preferring per-row `set_row_data` when the two
/// describe the same row identities in the same positions and falling back to `set_vec` on
/// any structural change. `id_of` extracts the identity that decision rests on.
pub fn apply_rows_keyed<T, F>(vec_model: &VecModel<T>, new_rows: Vec<T>, id_of: F)
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T) -> i32,
{
    let cur_count = vec_model.row_count();
    if cur_count == new_rows.len() {
        // One pass decides both questions: the id check needs each current row in hand
        // anyway, so comparing it in full also tells the write loop which rows to skip.
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

/// Empty a `VecModel<T>`-backed model in place, logging a downcast miss under `label`.
/// Section-leave teardown uses this so the model's `SharedString`s drop on the same UI
/// tick as the Image-property release.
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
