//! `Queue.*` callback wiring: a row's play, the drags that move rows, a drop from another list,
//! the row menu's favourite and rating, and the picks. Removals are `removal`'s.

use std::rc::Rc;

use slint::{ComponentHandle, Weak};

use super::publish_picks;
use crate::ui::callbacks::card_actions::spawn_with_track_ids;
use crate::ui::callbacks::collect_track_ids;
use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::drag;
use crate::ui::paged_rows::queue_rows::QueueRows;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_core::utils::block_move;
use melodia_ui::{AppWindow, Queue, QueueRow};

pub(super) fn wire_callbacks(ui: &AppWindow, state: &AppState, rows: &Rc<QueueRows>) {
    wire_skip_to(ui, state);
    wire_moves(ui, state, rows);
    wire_drop_in(ui, state, rows);
    wire_row_flags(ui, state);
    wire_selection(ui, rows);
}

/// A row's play, which reports nothing back: the skip lands on `sinks.queue` and the subscriber
/// shows it from there.
fn wire_skip_to(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Queue>().on_skip_to(move |idx| {
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

/// The two drags that move rows: one row, or every picked row gathered at a slot.
fn wire_moves(ui: &AppWindow, state: &AppState, rows: &Rc<QueueRows>) {
    let queue = ui.global::<Queue>();

    {
        let (s, rows) = (state.clone(), rows.clone());
        queue.on_reorder(move |from, to| {
            let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                return;
            };
            send_move(&s, &rows, vec![from], to);
        });
    }

    {
        let (s, rows) = (state.clone(), rows.clone());
        queue.on_move_picks(move |slot| {
            let Ok(slot) = usize::try_from(slot) else {
                return;
            };
            let picked = rows.picked_rows();
            let to = block_move::landing(&picked, slot);
            send_move(&s, &rows, picked, to);
        });
    }

    let rows = rows.clone();
    queue.on_picked_run(move || drag::lifted_run(&rows.picked_rows()));
}

/// Moves `moved` on show first, so the queue the engine publishes back is the one already on show
/// and every other row keeps its delegate, then has the engine move them.
fn send_move(state: &AppState, rows: &QueueRows, moved: Vec<usize>, to: usize) {
    if !rows.move_rows(&moved, to) {
        return;
    }
    let s = state.clone();
    state.runtime.spawn(async move {
        if let Err(e) = library::queue::queue_move(&s, &moved, to) {
            log::warn!("queue_move({} rows → {to}): {}", moved.len(), describe(&e));
        }
    });
}

/// Another list's drag let go over the rows, resolved as Add to Queue on it would be and spliced
/// in ahead of the row it was let go above.
///
/// The row is named by its entry here, on the thread that drew it, since the resolve awaits the
/// database and the queue can move under a slot meanwhile.
fn wire_drop_in(ui: &AppWindow, state: &AppState, rows: &Rc<QueueRows>) {
    let (state, rows) = (state.clone(), rows.clone());
    ui.global::<Queue>().on_drop_in(move |kind, ids, slot| {
        let Ok(slot) = usize::try_from(slot) else {
            return;
        };
        let before = rows.entry_at(slot);
        spawn_with_track_ids(
            &state,
            "queue drop",
            &kind,
            &ids,
            move |state, track_ids| async move {
                library::queue::queue_insert_before(&state, before, track_ids).await
            },
        );
    });
}

/// The row menu's Add to Favorites and rating, over the clicked row or the picks: the write, then
/// the rows it covers patched, as on every track list. A removal from Favorites patches them
/// through `RowFlagMirrors`.
///
/// Each write reaches the playing track and bumps `library_changed`, which re-reads every row the
/// panel holds and the records the player starts from. The patch is what moves the row now rather
/// than a fetch later, and the favourite's is also what the menu reads its next direction from,
/// which would otherwise offer the add again and write nothing.
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
pub(in crate::ui) fn patch_rows(
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
fn wire_selection(ui: &AppWindow, rows: &Rc<QueueRows>) {
    let weak = ui.as_weak();
    let queue = ui.global::<Queue>();

    {
        let (rows, weak) = (rows.clone(), weak.clone());
        queue.on_toggle_select(move |idx, ctrl, shift| {
            let Some(ui) = weak.upgrade() else { return };
            rows.click(idx, ctrl, shift);
            publish_picks(&ui, &rows);
        });
    }

    {
        let (rows, weak) = (rows.clone(), weak.clone());
        queue.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            rows.clear_picks();
            publish_picks(&ui, &rows);
        });
    }

    {
        let rows = rows.clone();
        queue.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            rows.select_all();
            publish_picks(&ui, &rows);
        });
    }
}
