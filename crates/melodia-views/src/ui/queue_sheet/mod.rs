//! Queue bottom-sheet UI wiring. Owns the Slint `Queue` global's
//! callbacks, its rows, a [`QueueRows`] over the queue's ids, and
//! the `is_open` atomic that the winit `DroppedFile` filter consults
//! to decide whether to forward dropped paths into
//! `library::queue::queue_import_files`.
//!
//! **Picks are by track, as on every other list**: the queue can hold a track twice, and picking
//! either row picks both, which is what the batch entries then act on.

mod callbacks;
mod rows;

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::ui::list_selection::{NO_ANCHOR, write_selection_ids};
use crate::ui::paged_rows::queue_rows::{self, QueueRows};
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::{AppWindow, Queue};

/// Public handle returned by [`install`]. Surfaces the `is_open`
/// atomic to boot so it can be forwarded into
/// `window_chrome::set_queue_sheet_open` for the winit `DroppedFile`
/// filter.
pub struct QueueSheetHandles {
    pub is_open: Arc<AtomicBool>,
}

/// Wire `Queue` callbacks, install the row model, and spawn the
/// queue-rows watch subscriber.
///
/// Must run on the Slint event-loop thread.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    row_covers: &Arc<CoverThumbs>,
) -> Result<QueueSheetHandles, slint::EventLoopError> {
    let rows = QueueRows::new(queue_rows::fetcher(state));
    ui.global::<Queue>().set_rows(ModelRc::from(rows.clone()));

    // Persistent, so every later write mutates in place rather than swapping a
    // model out from under the rows reading it — the track lists' idiom.
    let selected_ids: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Queue>().set_selected_ids(ModelRc::from(selected_ids));

    // The id a shift-click ranges from, the track lists' anchor, kept here because `Queue` has no
    // property for it.
    let anchor = Rc::new(Cell::new(NO_ANCHOR));
    let is_open = Arc::new(AtomicBool::new(false));
    // Private so closing can drop it wholesale: clearing the row tier would yank covers the track
    // lists and the now-playing bar still need. Backed by it, so a cover both draw is one buffer.
    let queue_covers = Arc::new(CoverThumbs::backed_by(row_covers.clone()));

    // Lazy row covers, wired once — the `boot::ui_setup` `RowCovers` shape,
    // against this sheet's private tier.
    //
    // `generation` is both the token that re-runs the row bindings and the
    // "is this tier warm yet" flag, which is what keeps the first frame off
    // the decoder. The teardown rewinds it to 0 in the same guarded closure
    // that clears the cache, so 0 always coincides with an empty tier — and
    // an open facing one would otherwise have every row it mounts while the
    // sheet slides up miss and decode *on the UI thread*. Cache-only until
    // the warm-up bump; after it, a row scrolled into view decodes on demand
    // like every other list in the app. See the callback's declaration in
    // `globals/queue.slint`.
    {
        let covers = queue_covers.clone();
        ui.global::<Queue>().on_request_cover(move |path, generation| {
            let path = Some(path.as_str()).filter(|s| !s.is_empty());
            if generation == 0 { covers.get_cached_opt(path) } else { covers.get_or_load_opt(path) }
        });
    }

    callbacks::wire_callbacks(ui, state, &rows, &queue_covers, &anchor, &is_open);
    rows::spawn_queue_rows_subscriber(ui, state, rows, anchor, is_open.clone())?;

    // No install-time seed: the sheet is closed at startup, so the
    // subscriber is gated and the model stays empty (no covers decoded
    // for an off-screen overlay). `on_open_changed(true)` shows the queue
    // on the first open.

    Ok(QueueSheetHandles { is_open })
}

/// Make `picked` the sheet's picks, less any track the queue no longer holds: stamped on the rows,
/// and published as the count the row menu labels its batch entries with and the ids it hands
/// them.
///
/// **The count is rows and the ids are each once**, the same number everywhere except a queue
/// holding one track twice: both rows are picked, and both are what Remove takes, where a playlist
/// should get the track once. The ids go in queue order, which is the order Copy writes them in.
pub(super) fn apply_picks(ui: &AppWindow, rows: &QueueRows, picked: &HashSet<i32>) {
    let mut count = 0usize;
    let mut held = HashSet::new();
    let mut ids = Vec::with_capacity(picked.len());
    for id in rows.row_ids() {
        if !picked.contains(&id) {
            continue;
        }
        count += 1;
        if held.insert(id) {
            ids.push(id);
        }
    }
    rows.select(&held);
    let queue = ui.global::<Queue>();
    queue.set_selected_count(i32::try_from(count).unwrap_or(i32::MAX));
    write_selection_ids(&queue.get_selected_ids(), ids, |m| queue.set_selected_ids(m));
}

/// The sheet's picks, as `apply_picks` last published them.
pub(super) fn picked(ui: &AppWindow) -> HashSet<i32> {
    ui.global::<Queue>().get_selected_ids().iter().collect()
}

/// The positions of the rows `picked` names, every copy of a track queued twice included.
pub(super) fn picked_positions(rows: &QueueRows, picked: &HashSet<i32>) -> Vec<usize> {
    rows.ids()
        .iter()
        .enumerate()
        .filter(|&(_, &id)| picked.contains(&clamp_i64_to_i32(id)))
        .map(|(position, _)| position)
        .collect()
}
