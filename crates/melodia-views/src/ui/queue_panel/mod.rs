//! The queue panel: the `Queue` global's callbacks and a [`QueueRows`] over the queue's ids,
//! followed only while the panel is shown.
//!
//! **Picks are by queue entry, not by track**: the queue can hold a track twice, and each copy is
//! its own row to pick, drag and remove.

mod callbacks;
mod lifecycle;
mod removal;
mod rows;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::ui::grid_prewarm::nonempty_artwork_path;
use crate::ui::list_selection::write_selection_ids;
use crate::ui::paged_rows::queue_rows::{self, QueueRows};
use crate::ui::undo::UndoWindow;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::{AppWindow, Queue};

pub(in crate::ui) use callbacks::patch_rows;
pub use removal::{QueueClearUndo, QueueCutUndo, QueueReplaceUndo};

/// Installs the row model and wires the panel. UI thread.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    row_covers: &Arc<CoverThumbs>,
    undo: &Rc<UndoWindow>,
) -> Result<(), slint::EventLoopError> {
    let rows = QueueRows::new(queue_rows::fetcher(state));
    ui.global::<Queue>().set_rows(ModelRc::from(rows.clone()));

    // Installed once, as on the track lists, so a write mutates it under the rows reading it.
    let selected_ids: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Queue>().set_selected_ids(ModelRc::from(selected_ids));

    let shown = Arc::new(AtomicBool::new(false));
    // Private so hiding can drop it wholesale: clearing the row tier would yank covers the track
    // lists and the now-playing bar still need. Backed by it, so a cover both draw is one buffer.
    let queue_covers = Arc::new(CoverThumbs::backed_by(row_covers.clone()));

    // A miss borrows the row tier's buffer or schedules a decode, never decoding here; the
    // generation the notifier bumps brings a scheduled one back.
    {
        let covers = queue_covers.clone();
        ui.global::<Queue>().on_request_cover(move |path, _generation| {
            covers.get_or_schedule_opt(nonempty_artwork_path(path.as_str()))
        });
    }
    crate::ui::cover_generation::notify_on_decode(&queue_covers, ui, |app| {
        let queue = app.global::<Queue>();
        queue.set_covers_generation(queue.get_covers_generation().wrapping_add(1));
    });

    callbacks::wire_callbacks(ui, state, &rows);
    removal::wire(ui, state, &rows, undo);
    removal::follow_replaces(undo)?;
    lifecycle::wire(ui, state, &rows, &queue_covers, &shown);
    // No seed: the panel starts hidden, and its first show seats the queue.
    rows::spawn_queue_rows_subscriber(ui, state, rows, shown)?;
    Ok(())
}

/// Publish the panel's picks as the count the row menu labels its batch entries with and the ids
/// it hands them, in queue order, which is the order Copy writes them in.
pub(super) fn publish_picks(ui: &AppWindow, rows: &QueueRows) {
    let (count, ids) = rows.picked_tracks();
    let queue = ui.global::<Queue>();
    queue.set_selected_count(i32::try_from(count).unwrap_or(i32::MAX));
    write_selection_ids(&queue.get_selected_ids(), ids, |m| queue.set_selected_ids(m));
}

#[cfg(test)]
#[path = "tests/queue_panel_tests.rs"]
mod tests;
