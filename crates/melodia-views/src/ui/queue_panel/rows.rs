//! The `sinks.queue` and `library_changed` subscriber, and what showing a new queue owes the
//! panel's picks and its drag.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_compat::Compat;
use slint::ComponentHandle;

use super::publish_picks;
use crate::ui::paged_rows::queue_rows::QueueRows;
use melodia_app::state::AppState;
use melodia_engine::player::engine::queue::{QueueEntry, current_index_to_i32};
use melodia_engine::player::engine::state::lock_state;
use melodia_ui::{AppWindow, Drag, DragOrigin, Queue};

/// Follows the queue and the library while the panel is shown: a queue publish shows the queue,
/// and a library tick re-reads the rows held. Hidden, the teardown has released the rows, and the
/// next show seats the queue from `PlayerState`.
///
/// A publish is only the cue: what it shows is [`read_queue`]'s, the channel's view model
/// carrying no entries.
pub(super) fn spawn_queue_rows_subscriber(
    ui: &AppWindow,
    state: &AppState,
    rows: Rc<QueueRows>,
    shown: Arc<AtomicBool>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let state = state.clone();
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
            if !shown.load(Ordering::Relaxed) {
                continue;
            }
            let Some(ui) = weak.upgrade() else { break };
            if !queue_moved {
                rows.reread();
                continue;
            }
            show_queue(&ui, &rows, read_queue(&state, &rows));
        }
        log::debug!("ui::queue_panel rows subscriber stopped");
    }))?;
    Ok(())
}

/// The queue as the panel shows it, read in one lock so its ids and its entries describe one
/// queue.
pub(super) struct QueueRead {
    pub(super) ids: Arc<[i64]>,
    /// `None` where `ids` is the order already on show, whose entries the panel holds.
    entries: Option<Arc<[QueueEntry]>>,
    pub(super) index: i32,
}

/// Read the queue off the state rather than off `sinks.queue`'s view model, which carries no
/// entries (`QueueViewModel` says why). They are derived only where the order on show has moved:
/// a track change hands back the ids it showed.
pub(super) fn read_queue(state: &AppState, rows: &QueueRows) -> QueueRead {
    let player = lock_state(&state.player_state);
    let ids = player.queue.play_ids();
    let entries = (!rows.shows(&ids)).then(|| player.queue.play_entries());
    QueueRead { ids, entries, index: current_index_to_i32(player.queue.current_index) }
}

/// Show the queue `read` describes and point the panel at its current row.
///
/// The same entries in the same order repaint nothing, which is every track change and the
/// publish that follows a drag the panel already moved. Anything else resets the list, so a drag
/// still in flight is revoked (it goes on, the panel's source surviving its rows, but the index it
/// lifted may no longer name the same row), and picks of rows that left the queue go.
pub(super) fn show_queue(ui: &AppWindow, rows: &QueueRows, read: QueueRead) {
    let moved = read.entries.is_some_and(|entries| rows.show(read.ids, entries));
    ui.global::<Queue>().set_current_index(read.index);
    if !moved {
        return;
    }
    ui.global::<Drag>().invoke_revoke(DragOrigin::Queue);
    publish_picks(ui, rows);
}
