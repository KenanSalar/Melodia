//! The queue panel's wiring. Owns the Slint `Queue` global's
//! callbacks, its rows, a [`QueueRows`] over the queue's ids, and
//! the `shown` atomic its row subscriber and lifecycle are gated on.
//!
//! **Picks are by queue entry, not by track**: the queue can hold a track twice, and each copy is
//! its own row to pick, drag and remove.

mod callbacks;
mod lifecycle;
mod rows;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::ui::grid_prewarm::nonempty_artwork_path;
use crate::ui::list_selection::write_selection_ids;
use crate::ui::paged_rows::queue_rows::{self, QueueRows};
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::{AppWindow, Queue};

/// Wire `Queue` callbacks, install the row model, and spawn the
/// queue-rows watch subscriber.
///
/// Must run on the Slint event-loop thread.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    row_covers: &Arc<CoverThumbs>,
) -> Result<(), slint::EventLoopError> {
    let rows = QueueRows::new(queue_rows::fetcher(state));
    ui.global::<Queue>().set_rows(ModelRc::from(rows.clone()));

    // Persistent, so every later write mutates in place rather than swapping a
    // model out from under the rows reading it — the track lists' idiom.
    let selected_ids: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Queue>().set_selected_ids(ModelRc::from(selected_ids));

    let shown = Arc::new(AtomicBool::new(false));
    // Private so hiding can drop it wholesale: clearing the row tier would yank covers the track
    // lists and the now-playing bar still need. Backed by it, so a cover both draw is one buffer.
    let queue_covers = Arc::new(CoverThumbs::backed_by(row_covers.clone()));

    // Lazy row covers, wired once: the `boot::ui_setup` `RowCovers` shape, against this panel's
    // private tier. A miss borrows the row tier's buffer or schedules a decode, never decoding on
    // the UI thread, and the generation the notifier bumps is what brings a scheduled one back.
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
    lifecycle::wire(ui, state, &rows, &queue_covers, &shown);
    rows::spawn_queue_rows_subscriber(ui, state, rows, shown)?;

    // No install-time seed: the panel is hidden at startup, so the
    // subscriber is gated and the model stays empty (no covers decoded
    // for an off-screen panel). `on_shown_changed(true)` shows the queue
    // on the first show.

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
