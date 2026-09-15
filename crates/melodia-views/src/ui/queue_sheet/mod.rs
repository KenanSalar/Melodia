//! Queue bottom-sheet UI wiring. Owns the Slint `Queue` global's
//! callbacks, the row `VecModel`, the per-row selection shadow, and
//! the `is_open` atomic that the winit `DroppedFile` filter consults
//! to decide whether to forward dropped paths into
//! `library::queue::queue_import_files`.

mod callbacks;
mod links;
mod rows;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::{CoverThumbs, row_cover_size};
use melodia_core::entities::track::TrackSummary;
use melodia_ui::{AppWindow, Queue, QueueRow};

pub(crate) use rows::to_slint_queue_row;

/// Per-row selection snapshot, beside the summary the row was built from. Kept deliberately
/// minimal so the shadow is `Send + Sync` regardless of `SharedString`'s thread-safety
/// guarantees — those live on the UI side inside `Rc<VecModel<QueueRow>>`.
///
/// `source` is what lets a rebuild answer "did any row's content move" without building one, and
/// **the strong reference is the whole reason `Arc::ptr_eq` against it is exact.** A summary *is*
/// mutated in place where it is uniquely owned (`sync_track_summaries` reaches `queue.tracks`
/// through `Arc::make_mut`), so an address alone would compare equal to content that had moved
/// underneath it. Holding one here puts the refcount above one, which is what leaves `make_mut`
/// no arm but to clone — so a shadowed row that changed is always a different pointer.
///
/// `None` past the close teardown: the summaries are the larger half of what the shadow costs on
/// a queue the size of the library, and a queue replaced while the sheet is closed would leave
/// them pinned with nothing to draw them. The row-count term is what covers the reopen.
#[derive(Clone)]
pub(super) struct ShadowEntry {
    pub id: i64,
    pub selected: bool,
    pub source: Option<Arc<TrackSummary>>,
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
    ui.global::<Queue>().set_rows(ModelRc::from(queue_model.clone()));

    // Persistent, so every later write mutates in place rather than swapping a
    // model out from under the rows reading it — the track lists' idiom.
    let selected_ids: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Queue>().set_selected_ids(ModelRc::from(selected_ids));

    let shadow: Arc<Mutex<Vec<ShadowEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let link_cache: links::LinkCache = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let anchor: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let is_open = Arc::new(AtomicBool::new(false));
    // The queue sheet's *own* row-tier cover cache — deliberately NOT the
    // shared `cover_thumbs`. Private so it can be dropped wholesale when
    // the sheet closes (clearing the shared cache would yank covers the
    // Tracks / Browse views and the now-playing bar still need).
    let queue_covers = Arc::new(CoverThumbs::new());
    // Deferred for the reason the grid tiers are: `install` runs long before
    // `app.show()`, and `scale_factor` answers 1.0 until the window is on a
    // monitor. The sheet can't be open yet, so the tier is empty either way.
    let tune_covers = queue_covers.clone();
    let tune_weak = ui.as_weak();
    if let Err(e) = slint::invoke_from_event_loop(move || {
        let Some(ui) = tune_weak.upgrade() else {
            return;
        };
        tune_covers.set_thumb_size(row_cover_size(f64::from(ui.window().scale_factor())));
    }) {
        log::warn!("Failed to schedule queue-cover display tuning: {e}");
    }

    // Lazy row covers, wired once — the `boot::ui_setup` `RowCovers` shape,
    // against this sheet's private tier.
    //
    // `generation` is both the token that re-runs the row bindings and the
    // "is this tier warm yet" flag, which is what keeps the first frame off
    // the decoder. The teardown rewinds it to 0 in the same guarded closure
    // that clears the cache, so 0 always coincides with an empty tier — and
    // an open facing one would otherwise have every row it mounts
    // synchronously under `on_open_changed` miss and decode *on the UI
    // thread*, mid-slide-up, which is the one thing the synchronous row build
    // exists to avoid. Cache-only until the warm-up bump; after it, a row
    // scrolled into view decodes on demand like every other list in the app.
    // See the callback's declaration in `globals/queue.slint`.
    {
        let covers = queue_covers.clone();
        ui.global::<Queue>().on_request_cover(move |path, generation| {
            let path = Some(path.as_str()).filter(|s| !s.is_empty());
            if generation == 0 { covers.get_cached_opt(path) } else { covers.get_or_load_opt(path) }
        });
    }

    callbacks::wire_callbacks(
        ui,
        state,
        &queue_model,
        &queue_covers,
        &shadow,
        &link_cache,
        &anchor,
        &is_open,
    );
    rows::spawn_queue_rows_subscriber(ui, state, queue_model, shadow, link_cache, is_open.clone())?;

    // No install-time seed: the sheet is closed at startup, so the
    // subscriber is gated and the model stays empty (no covers decoded
    // for an off-screen overlay). `on_open_changed(true)` does a fresh
    // rebuild from PlayerState on the first open.

    Ok(QueueSheetHandles { is_open })
}

/// Push the selection from the shadow into the Slint `Queue` global: the
/// count the header's "{n} selected" pill reads, and the id list the row
/// context menu hands to a batch action.
///
/// **The count is rows and the ids are deduped**, which is the same number
/// everywhere except a queue holding one track twice. Selection is preserved
/// by track id across a rebuild, so ticking either instance marks both — and
/// an undeduped list would then add that track to a playlist twice. The menu's
/// multi-mode gate reads the count, so the labels still say what the pill says.
pub(super) fn push_selection(weak: &Weak<AppWindow>, shadow: &[ShadowEntry]) {
    if let Some(ui) = weak.upgrade() {
        write_selection(&ui, shadow);
    }
}

/// [`push_selection`] against a live handle, for the rebuild path that already
/// holds one.
pub(super) fn write_selection(ui: &AppWindow, shadow: &[ShadowEntry]) {
    let selected = shadow.iter().filter(|e| e.selected);
    let count = i32::try_from(selected.clone().count()).unwrap_or(i32::MAX);

    let mut seen: HashSet<i64> = HashSet::new();
    let ids: Vec<i32> = selected
        .filter(|e| seen.insert(e.id))
        .map(|e| i32::try_from(e.id).unwrap_or(i32::MAX))
        .collect();

    let queue = ui.global::<Queue>();
    queue.set_selected_count(count);
    crate::ui::list_selection::write_selection_ids(&queue.get_selected_ids(), ids, |m| {
        queue.set_selected_ids(m);
    });
}
