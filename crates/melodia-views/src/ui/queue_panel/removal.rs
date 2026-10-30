//! Taking rows out of the queue, and Clear, each at once with an undo. The panel's ✕, its menu,
//! Delete and Up Next's menu all arrive here.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::ui::paged_rows::queue_rows::QueueRows;
use crate::ui::undo::{Removal, UndoWindow};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_engine::player::engine::queue::{ClearedQueue, QueueCut, QueueEntry};
use melodia_engine::player::engine::state::lock_state;
use melodia_ui::{AppWindow, Queue};

/// What a run of removals from the queue took.
pub struct QueueCutUndo {
    cuts: Vec<QueueCut>,
}

impl QueueCutUndo {
    pub fn absorb(&mut self, newer: Self) {
        self.cuts.extend(newer.cuts);
    }

    pub fn count(&self) -> usize {
        self.cuts.iter().map(QueueCut::len).sum()
    }

    /// No refresh owed: the panel and Up Next follow the queue's publish.
    pub fn restore(self, state: &AppState) {
        library::queue::queue_restore_cuts(state, self.cuts);
    }
}

/// What a run of Clears took.
pub struct QueueClearUndo {
    cleared: Vec<ClearedQueue>,
}

impl QueueClearUndo {
    pub fn absorb(&mut self, newer: Self) {
        self.cleared.extend(newer.cleared);
    }

    pub fn restore(self, state: &AppState) {
        library::queue::queue_restore_cleared(state, self.cleared);
    }
}

/// Each removal is a short state lock, so it runs here on the UI thread, where the undo slot lives,
/// rather than hopping to the runtime and back.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rows: &Rc<QueueRows>, undo: &Rc<UndoWindow>) {
    let remover = Remover { state: state.clone(), undo: undo.clone() };
    let queue = ui.global::<Queue>();

    {
        let (remover, rows) = (remover.clone(), rows.clone());
        queue.on_remove(move |row| {
            let entry = usize::try_from(row).ok().and_then(|row| rows.entry_at(row));
            remover.cut(entry.as_slice());
        });
    }

    {
        let (remover, rows) = (remover.clone(), rows.clone());
        queue.on_remove_selected(move || {
            let entries: Vec<QueueEntry> =
                rows.picked_rows().into_iter().filter_map(|row| rows.entry_at(row)).collect();
            remover.cut(&entries);
        });
    }

    {
        let remover = remover.clone();
        queue.on_remove_upcoming(move |index, track_id| {
            let entry = remover.upcoming_entry(index, track_id);
            remover.cut(entry.as_slice());
        });
    }

    queue.on_clear_queue(move || remover.clear());
}

#[derive(Clone)]
struct Remover {
    state: AppState,
    undo: Rc<UndoWindow>,
}

impl Remover {
    fn cut(&self, entries: &[QueueEntry]) {
        if entries.is_empty() {
            return;
        }
        if let Some(cut) = library::queue::queue_remove(&self.state, entries) {
            self.undo.offer(Removal::QueueRows(QueueCutUndo { cuts: vec![cut] }));
        }
    }

    fn clear(&self) {
        if let Some(cleared) = library::queue::queue_clear(&self.state) {
            self.undo.offer(Removal::QueueCleared(QueueClearUndo { cleared: vec![cleared] }));
        }
    }

    /// The row Up Next drew at play-order `index`, only while it still holds `track_id`: Up Next
    /// draws from a publish, which can trail the queue.
    fn upcoming_entry(&self, index: i32, track_id: i32) -> Option<QueueEntry> {
        let index = usize::try_from(index).ok()?;
        let player = lock_state(&self.state.player_state);
        let drawn = player.queue.id_at(index) == Some(i64::from(track_id));
        drawn.then(|| player.queue.entry_at(index)).flatten()
    }
}
