//! The `sinks.queue` and `library_changed` subscriber, and what showing a new queue owes the
//! sheet's picks and drag state.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_compat::Compat;
use slint::ComponentHandle;

use super::{apply_picks, picked};
use crate::ui::list_selection::NO_ANCHOR;
use crate::ui::paged_rows::queue_rows::QueueRows;
use crate::ui::paged_rows::{Keyset, ReadStamp};
use melodia_app::state::AppState;
use melodia_engine::player::engine::state::QueueViewModel;
use melodia_ui::{AppWindow, Queue};

/// Follow the queue and the library while the sheet is open: each queue publish shows the queue,
/// and each library tick reads the rows the sheet holds again.
///
/// Gated on the sheet being on screen: closed, nobody sees the rows and the teardown has released
/// them, so following would fetch rows nothing reads. The next `on_open_changed(true)` shows the
/// queue from `PlayerState`.
pub(super) fn spawn_queue_rows_subscriber(
    ui: &AppWindow,
    state: &AppState,
    rows: Rc<QueueRows>,
    anchor: Rc<Cell<i32>>,
    is_open: Arc<AtomicBool>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let mut queue_rx = state.sinks.queue.subscribe();
    let mut library_rx = state.library_changed.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            let queue_moved = tokio::select! {
                changed = queue_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    true
                }
                changed = library_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    false
                }
            };
            if !is_open.load(Ordering::Relaxed) {
                continue;
            }
            let Some(ui) = weak.upgrade() else { break };
            if !queue_moved {
                reread_rows(&rows);
                continue;
            }
            let snapshot = queue_rx.borrow_and_update().clone();
            if let Some(qvm) = snapshot {
                show_queue(&ui, &rows, &anchor, &qvm);
            }
        }
        log::debug!("ui::queue_panel rows subscriber stopped");
    }))?;
    Ok(())
}

/// Show the queue `qvm` describes and point the sheet at its current row.
///
/// The same ids in the same order repaint nothing, which is every track change and the publish
/// that follows a drag the sheet already moved. Anything else resets the list, so a drag still in
/// flight is aborted (its indices no longer describe the queue, and the reset destroys the row
/// holding the pointer grab, which left set keeps the source ghosted and the drop line stranded),
/// and picks of tracks that left the queue go.
pub(super) fn show_queue(
    ui: &AppWindow,
    rows: &QueueRows,
    anchor: &Cell<i32>,
    qvm: &QueueViewModel,
) {
    let moved = *rows.ids() != *qvm.ids;
    rows.show(Keyset { ids: Arc::clone(&qvm.ids), seed: Vec::new(), read: ReadStamp::now() });
    let queue = ui.global::<Queue>();
    queue.set_current_index(qvm.queue_index);
    if !moved {
        return;
    }
    queue.set_drag_source(-1);
    queue.set_drop_slot(-1);
    keep_picks_within_queue(ui, rows, anchor);
}

/// Drop the picks, and the anchor, of tracks the queue no longer holds.
fn keep_picks_within_queue(ui: &AppWindow, rows: &QueueRows, anchor: &Cell<i32>) {
    let picked = picked(ui);
    if picked.is_empty() && anchor.get() == NO_ANCHOR {
        return;
    }
    let row_ids = rows.row_ids();
    if !row_ids.contains(&anchor.get()) {
        anchor.set(NO_ANCHOR);
    }
    apply_picks(ui, rows, &picked);
}

/// Read every row the sheet holds again, those on screen first, after a library change that may
/// have rewritten any of them. The ids are the queue's rather than a query's, so the stamp they
/// are read again against is now.
fn reread_rows(rows: &QueueRows) {
    rows.show(Keyset { ids: rows.ids(), seed: Vec::new(), read: ReadStamp::now() });
    rows.reread();
}
