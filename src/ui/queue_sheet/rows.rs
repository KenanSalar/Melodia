//! `sinks.queue` subscriber + row-rebuild helper + the `QueueRow`
//! shape converter (shared with Now Playing's Up Next list).

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_compat::Compat;
use parking_lot::Mutex;
use slint::{ComponentHandle, Image, Model, VecModel, Weak};

use super::ShadowEntry;
use crate::entities::track::TrackSummary;
use crate::media::cover_thumbs::CoverThumbs;
use crate::player::state::QueueViewModel;
use crate::state::AppState;
use crate::ui::tracks::format_duration_ms;
use crate::{AppWindow, Queue, QueueRow};

/// Subscribe to `state.sinks.queue` and rebuild the row model on
/// every mutation. Preserves the per-row `selected` bit by track id
/// so reorders don't drop selection.
pub(super) fn spawn_queue_rows_subscriber(
    ui: &AppWindow,
    state: &AppState,
    queue_covers: Arc<CoverThumbs>,
    queue_model: Rc<VecModel<QueueRow>>,
    shadow: Arc<Mutex<Vec<ShadowEntry>>>,
    is_open: Arc<AtomicBool>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let mut rx = state.sinks.queue.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = rx.borrow_and_update().clone();
            // Gated on the sheet being on screen: while it's closed,
            // nobody sees the rows and the model + cover cache have been
            // released — rebuilding would just re-decode covers for an
            // off-screen overlay. The next `on_open_changed(true)` does a
            // fresh rebuild from `PlayerState`.
            if !is_open.load(Ordering::Relaxed) {
                continue;
            }
            let Some(ui) = weak.upgrade() else { break };
            let Some(qvm) = snapshot else { continue };
            rebuild_rows(&ui, &queue_model, &shadow, &qvm, |p| {
                queue_covers.get_or_load_opt(p)
            });
        }
        log::debug!("ui::queue_sheet rows subscriber stopped");
    }))?;
    Ok(())
}

/// Rebuild `queue_model` and the per-row selection shadow from a
/// `QueueViewModel` snapshot, then update `Queue.current_index` and
/// `Queue.selected_count`. Called from the watch-channel subscriber on
/// every mutation (while the sheet is open) and from
/// `on_open_changed(true)` on the first open / each subsequent reopen.
///
/// Takes `&VecModel<QueueRow>` instead of `&Rc<VecModel<…>>` so the
/// upgrade-in-event-loop callback (which receives the model via
/// `downcast_ref` rather than carrying an `Rc` across the await) can
/// share this entry point with the watch subscriber.
///
/// `img_for` decides where each row's cover comes from — the open
/// path's *skeleton* pass passes a cache-only lookup so rows render
/// synchronously without blocking the slide-up animation on decode;
/// every other caller passes `get_or_load_opt` which decodes on miss.
pub(super) fn rebuild_rows(
    ui: &AppWindow,
    queue_model: &VecModel<QueueRow>,
    shadow: &Arc<Mutex<Vec<ShadowEntry>>>,
    qvm: &QueueViewModel,
    img_for: impl Fn(Option<&str>) -> Image,
) {
    // Snapshot the old selection bits into a map so the per-row lookup
    // below is O(1) — a linear `.find()` per row made this O(n²) overall,
    // re-run on every queue mutation (including each frame of a drag).
    let old_sel: std::collections::HashMap<i64, bool> = shadow
        .lock()
        .iter()
        .map(|e| (e.id, e.selected))
        .collect();
    let mut new_shadow: Vec<ShadowEntry> = Vec::with_capacity(qvm.queue_tracks.len());
    let mut new_rows: Vec<QueueRow> = Vec::with_capacity(qvm.queue_tracks.len());
    for t in &qvm.queue_tracks {
        let selected = old_sel.get(&t.id).copied().unwrap_or(false);
        new_shadow.push(ShadowEntry {
            id: t.id,
            selected,
        });
        let cover_img = img_for(t.artwork_path.as_deref());
        new_rows.push(to_slint_queue_row(t.as_ref(), cover_img, selected));
    }

    crate::ui::model_diff::apply_rows_keyed(queue_model, new_rows, |r| r.id);
    let selected_count =
        i32::try_from(new_shadow.iter().filter(|e| e.selected).count()).unwrap_or(i32::MAX);
    *shadow.lock() = new_shadow;
    let queue = ui.global::<Queue>();
    queue.set_current_index(qvm.queue_index);
    queue.set_selected_count(selected_count);
}

/// Build a `QueueRow` from a `TrackSummary` plus an already-resolved
/// cover image. `pub(crate)` because the full-screen Now Playing view's
/// "Up Next" list (`ui::now_playing`) reuses the exact same row shape —
/// it always passes `selected: false`. Callers compute `cover_img`
/// themselves so the queue sheet's two-phase render (cache-only
/// skeleton, then warmed rebuild) can swap the lookup policy without
/// duplicating the row-shape mapping.
/// Surgically flip `is_favorite` on every visible queue row whose `id`
/// matches. Mirrors `crate::ui::tracks::fetch::apply_row_favorite`. We
/// do this rather than emitting via `sinks.queue` because
/// `with_state_emit` only publishes the queue VM when `queue.version`
/// changed — a favorite toggle isn't a queue mutation, so bumping the
/// version would trigger a full row rebuild for a single-bit change.
pub(crate) fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let rows = ui.global::<Queue>().get_rows();
        let Some(vm) = rows.as_any().downcast_ref::<VecModel<QueueRow>>() else {
            return;
        };
        for i in 0..vm.row_count() {
            let Some(mut r) = vm.row_data(i) else { continue };
            if i64::from(r.id) == id && r.is_favorite != fav {
                r.is_favorite = fav;
                vm.set_row_data(i, r);
            }
        }
    });
}

pub(crate) fn to_slint_queue_row(t: &TrackSummary, cover_img: Image, selected: bool) -> QueueRow {
    let display_duration = format_duration_ms(t.duration_ms.max(0));
    QueueRow {
        id: i32::try_from(t.id).unwrap_or(i32::MAX),
        title: t.title.as_str().into(),
        artist: t.artist.as_deref().unwrap_or("").into(),
        artwork_path: t.artwork_path.as_deref().unwrap_or("").into(),
        cover_img,
        display_duration: display_duration.into(),
        selected,
        is_favorite: t.is_favorite,
    }
}
