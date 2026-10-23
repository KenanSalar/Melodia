//! `Queue.*` callback wiring: the transport rows, a drop from another list, the row menu's
//! favourite and rating, and the picks.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use slint::{ComponentHandle, Weak};

use super::{apply_picks, picked, picked_positions};
use crate::ui::callbacks::card_actions::spawn_with_track_ids;
use crate::ui::callbacks::collect_track_ids;
use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::list_selection::{NO_ANCHOR, compute_click_selection};
use crate::ui::paged_rows::queue_rows::QueueRows;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Queue, QueueRow};

pub(super) fn wire_callbacks(
    ui: &AppWindow,
    state: &AppState,
    rows: &Rc<QueueRows>,
    anchor: &Rc<Cell<i32>>,
) {
    wire_transport(ui, state, rows);
    wire_drop_in(ui, state);
    wire_row_flags(ui, state);
    wire_selection(ui, rows, anchor);
}

/// The five rows that only ask the engine for something.
///
/// None of them reports anything back: the mutation lands on `sinks.queue` and the subscriber
/// shows it from there.
fn wire_transport(ui: &AppWindow, state: &AppState, rows: &Rc<QueueRows>) {
    let queue = ui.global::<Queue>();

    {
        let s = state.clone();
        let rows = rows.clone();
        queue.on_reorder(move |from, to| {
            let Ok(from) = usize::try_from(from) else {
                return;
            };
            let Ok(to) = usize::try_from(to) else { return };
            if from == to {
                return;
            }
            // Moved here first, so the queue the engine publishes back is the one already on
            // show and every other row keeps its delegate.
            rows.move_row(from, to);
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_move(&s_inner, from, to) {
                    log::warn!("queue_move({from},{to}): {}", describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_remove(move |idx| {
            let Ok(idx) = usize::try_from(idx) else {
                return;
            };
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_remove(&s_inner, idx) {
                    log::warn!("queue_remove({idx}): {}", describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        let rows = rows.clone();
        let weak = ui.as_weak();
        queue.on_remove_selected(move || {
            let Some(ui) = weak.upgrade() else { return };
            let indices = picked_positions(&rows, &picked(&ui));
            if indices.is_empty() {
                return;
            }
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_remove_batch(&s_inner, &indices) {
                    log::warn!("queue_remove_batch({} rows): {}", indices.len(), describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_skip_to(move |idx| {
            let Ok(idx) = usize::try_from(idx) else {
                return;
            };
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_skip_to_index(&s_inner, idx).await {
                    log::warn!("queue_skip_to_index({idx}): {}", describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_clear_queue(move || {
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_clear(&s_inner) {
                    log::warn!("queue_clear: {}", describe(&e));
                }
            });
        });
    }
}

/// Another list's drag let go over the rows, resolved as Add to Queue on it would be and spliced
/// in at the slot it was let go at.
fn wire_drop_in(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    ui.global::<Queue>().on_drop_in(move |kind, ids, slot| {
        let slot = usize::try_from(slot).unwrap_or(0);
        spawn_with_track_ids(
            &state,
            "queue drop",
            &kind,
            &ids,
            move |state, track_ids| async move {
                library::queue::queue_insert_at(&state, slot, track_ids).await
            },
        );
    });
}

/// The row menu's favourite and rating, over the clicked row or the picks: the write, then the
/// rows it covers patched, as on every track list.
///
/// Each write reaches the playing track and bumps `library_changed`, which re-reads every row the
/// panel holds and the records the player starts from. The patch is what moves the row now rather
/// than a fetch later, and the favourite's is also what the menu reads its next direction from,
/// which would otherwise compute `!false` again and write nothing.
fn wire_row_flags(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Queue>();
    let weak = ui.as_weak();
    wire_row_flag!(g, on_toggle_row_favorite, state, "queue::set_favorite",
    library::favorites::set_favorite, collect_track_ids,
    captures: [weak],
    after: |id_vec, fav| {
        patch_rows(&weak, id_vec, move |row| row.is_favorite = fav);
    });
    wire_row_flag!(g, on_set_row_rating, state, "queue::set_rating",
    library::ratings::set_rating, collect_track_ids,
    captures: [weak],
    after: |id_vec, rating| {
        patch_rows(&weak, id_vec, move |row| row.rating = rating);
    });
}

/// Apply `edit` to the cached rows of `ids`, every copy of a track queued twice included.
fn patch_rows(
    weak: &Weak<AppWindow>,
    ids: Vec<i64>,
    edit: impl Fn(&mut QueueRow) + Send + 'static,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<Queue>().get_rows();
        let Some(rows) = QueueRows::installed(&model, "queue row patch") else {
            return;
        };
        for id in ids {
            rows.patch(id, &edit);
        }
    });
}

/// The three selection rows, each over the shared click semantics the track lists use.
fn wire_selection(ui: &AppWindow, rows: &Rc<QueueRows>, anchor: &Rc<Cell<i32>>) {
    let weak = ui.as_weak();
    let queue = ui.global::<Queue>();

    {
        let (rows, anchor, weak) = (rows.clone(), anchor.clone(), weak.clone());
        queue.on_toggle_select(move |idx, ctrl, shift| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(id) = usize::try_from(idx).ok().and_then(|at| rows.ids().get(at).copied())
            else {
                return;
            };
            let (picks, next_anchor) = compute_click_selection(
                anchor.get(),
                picked(&ui).into_iter().collect(),
                || rows.row_ids(),
                idx,
                clamp_i64_to_i32(id),
                shift,
                ctrl,
            );
            anchor.set(next_anchor);
            apply_picks(&ui, &rows, &picks.into_iter().collect());
        });
    }

    {
        let (rows, anchor, weak) = (rows.clone(), anchor.clone(), weak.clone());
        queue.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            apply_picks(&ui, &rows, &HashSet::new());
            // Only the clear drops the anchor: Select All leaves whatever a click last established,
            // so a following Shift-click still extends from where the user was.
            anchor.set(NO_ANCHOR);
        });
    }

    {
        let rows = rows.clone();
        queue.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            apply_picks(&ui, &rows, &rows.row_ids().into_iter().collect());
        });
    }
}
