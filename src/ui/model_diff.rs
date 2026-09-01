//! Reusable `VecModel<T>` swap helpers. [`apply_rows_keyed`] prefers per-row updates over
//! full-model replacement; [`permute_rows_by_id`] and [`clear_vec_model`] are the two whose
//! job the replacement is.
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

use std::collections::HashMap;

use slint::{Model, ModelRc, VecModel};

/// Apply `new_rows` to `vec_model`, preferring per-row `set_row_data` when the two
/// describe the same row identities in the same positions and falling back to `set_vec` on
/// any structural change. `id_of` extracts the identity that decision rests on.
///
/// Returns whether the model was **reset** — i.e. whether row positions moved. Callers holding
/// anything keyed on a row index (a shift-range anchor, a drop slot) may keep it on `false`,
/// where the same ids sit where they did; on `true` those indices name different rows now.
pub fn apply_rows_keyed<T, F>(vec_model: &VecModel<T>, new_rows: Vec<T>, id_of: F) -> bool
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
            return false;
        }
    }
    vec_model.set_vec(new_rows);
    true
}

/// Reorder a `VecModel<T>`-backed model into `order`, matching rows by the identity `id_of`
/// extracts. Ids naming no row are dropped, and rows `order` doesn't name go with them.
///
/// Moving each existing row rather than rebuilding it is the point — no decode, no `format!`,
/// no `SharedString` alloc — and each row carries its `selected` flag along, which is why the
/// four detail views only re-sync selection defensively afterwards. `set_vec` on purpose: a
/// permutation is structural, so [`apply_rows_keyed`] would fall back to one anyway.
pub fn permute_rows_by_id<T, F>(model: &ModelRc<T>, order: &[i32], id_of: F)
where
    T: Clone + 'static,
    F: Fn(&T) -> i32,
{
    let Some(vec_model) = model.as_any().downcast_ref::<VecModel<T>>() else {
        // The caller has already sorted its Rust caches, and every index-to-track lookup it
        // serves reads those, so a silent bail leaves the two disagreeing rather than idle.
        log::warn!("permute_rows_by_id: VecModel downcast failed, rows left in the old order");
        return;
    };
    let mut by_id: HashMap<i32, T> = HashMap::with_capacity(vec_model.row_count());
    for i in 0..vec_model.row_count() {
        if let Some(row) = vec_model.row_data(i) {
            by_id.insert(id_of(&row), row);
        }
    }
    let reordered: Vec<T> = order.iter().filter_map(|id| by_id.remove(id)).collect();
    vec_model.set_vec(reordered);
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
