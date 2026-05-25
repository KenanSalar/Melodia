//! Queue bottom-sheet UI wiring. Owns the Slint `Queue` global's
//! callbacks, the row `VecModel`, the per-row selection shadow, and
//! the `is_open` atomic that the winit `DroppedFile` filter consults
//! to decide whether to forward dropped paths into
//! `library::queue::queue_import_files`.

mod callbacks;
mod rows;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::media::cover_thumbs::CoverThumbs;
use crate::state::AppState;
use crate::{AppWindow, Queue, QueueRow};

pub(crate) use rows::to_slint_queue_row;

/// Per-row selection snapshot. Kept deliberately minimal so the
/// shadow is `Send + Sync` regardless of `slint::Image` /
/// `SharedString` thread-safety guarantees — those live on the UI
/// side inside `Rc<VecModel<QueueRow>>`.
#[derive(Clone, Copy)]
pub(super) struct ShadowEntry {
    pub id: i64,
    pub selected: bool,
}

/// Public handle returned by [`install`]. Surfaces the `is_open`
/// atomic to `main.rs` so it can be forwarded into
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
) -> Result<QueueSheetHandles, slint::EventLoopError> {
    let queue_model: Rc<VecModel<QueueRow>> = Rc::new(VecModel::default());
    ui.global::<Queue>()
        .set_rows(ModelRc::from(queue_model.clone()));

    let shadow: Arc<Mutex<Vec<ShadowEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let anchor: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let is_open = Arc::new(AtomicBool::new(false));
    // The queue sheet's *own* 72 px cover cache — deliberately NOT the
    // shared `cover_thumbs`. Private so it can be dropped wholesale when
    // the sheet closes (clearing the shared cache would yank covers the
    // Tracks / Browse views and the now-playing bar still need).
    let queue_covers = Arc::new(CoverThumbs::new());

    callbacks::wire_callbacks(
        ui,
        state,
        &queue_model,
        &queue_covers,
        &shadow,
        &anchor,
        &is_open,
    );
    rows::spawn_queue_rows_subscriber(
        ui,
        state,
        queue_covers,
        queue_model,
        shadow,
        is_open.clone(),
    )?;

    // No install-time seed: the sheet is closed at startup, so the
    // subscriber is gated and the model stays empty (no covers decoded
    // for an off-screen overlay). `on_open_changed(true)` does a fresh
    // rebuild from PlayerState on the first open.

    Ok(QueueSheetHandles { is_open })
}

/// Push the current selected-count from the shadow into the Slint
/// `Queue` global so the header's "Remove selected (N)" vs "Clear
/// queue" swap reacts. Cheap: a single iteration over the shadow.
pub(super) fn push_selected_count(weak: &Weak<AppWindow>, shadow: &[ShadowEntry]) {
    let count = i32::try_from(shadow.iter().filter(|e| e.selected).count()).unwrap_or(i32::MAX);
    if let Some(ui) = weak.upgrade() {
        ui.global::<Queue>().set_selected_count(count);
    }
}
