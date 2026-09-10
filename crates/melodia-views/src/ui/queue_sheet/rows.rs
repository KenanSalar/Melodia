//! `sinks.queue` subscriber + row-rebuild helper + the `QueueRow`
//! shape converter (shared with Now Playing's Up Next list).

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_compat::Compat;
use parking_lot::Mutex;
use slint::{ComponentHandle, Model, VecModel, Weak};

use super::ShadowEntry;
use super::links::{self, LinkCache, RowLinks};
use crate::ui::model_patch::patch_rows_where;
use crate::ui::tracks::format_duration_ms;
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackSummary;
use melodia_engine::player::engine::state::QueueViewModel;
use melodia_ui::{AppWindow, Queue, QueueRow};

/// Subscribe to `state.sinks.queue` and rebuild the row model on
/// every mutation. Preserves the per-row `selected` bit by track id
/// so reorders don't drop selection.
pub(super) fn spawn_queue_rows_subscriber(
    ui: &AppWindow,
    state: &AppState,
    queue_model: Rc<VecModel<QueueRow>>,
    shadow: Arc<Mutex<Vec<ShadowEntry>>>,
    link_cache: LinkCache,
    is_open: Arc<AtomicBool>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let state = state.clone();
    let mut rx = state.sinks.queue.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = rx.borrow_and_update().clone();
            // Gated on the sheet being on screen: while it's closed,
            // nobody sees the rows and the model + cover cache have been
            // released, so rebuilding would churn a model nothing reads.
            // The next `on_open_changed(true)` does a fresh rebuild from
            // `PlayerState`.
            if !is_open.load(Ordering::Relaxed) {
                continue;
            }
            let Some(ui) = weak.upgrade() else { break };
            let Some(qvm) = snapshot else { continue };
            let missing = rebuild_rows(&ui, &queue_model, &shadow, &link_cache, &qvm);
            // Only tracks the queue has never carried this open — a reorder or
            // a removal resolves nothing and asks nothing.
            links::fetch_missing(&state, &weak, &link_cache, &is_open, missing);
        }
        log::debug!("ui::queue_sheet rows subscriber stopped");
    }))?;
    Ok(())
}

/// Rebuild `queue_model` and the per-row selection shadow from a
/// `QueueViewModel` snapshot, then update `Queue.current_index` and the
/// selection. Called from the watch-channel subscriber on every mutation
/// (while the sheet is open) and from `on_open_changed(true)` on the first
/// open / each subsequent reopen.
///
/// Returns the track ids whose FK linkage the cache couldn't answer, for
/// `links::fetch_missing` — empty on every rebuild after the first, that
/// function claiming what it asks for, so the caller's follow-up is free where
/// nothing new arrived.
///
/// Takes `&VecModel<QueueRow>` instead of `&Rc<VecModel<…>>` so the
/// upgrade-in-event-loop callback (which receives the model via
/// `downcast_ref` rather than carrying an `Rc` across the await) can
/// share this entry point with the watch subscriber.
///
/// Covers aren't touched here — each row resolves its own through
/// `Queue.request-cover` when it's actually on screen, which is what
/// keeps this cheap on a queue the size of the library.
pub(super) fn rebuild_rows(
    ui: &AppWindow,
    queue_model: &VecModel<QueueRow>,
    shadow: &Arc<Mutex<Vec<ShadowEntry>>>,
    link_cache: &LinkCache,
    qvm: &QueueViewModel,
) -> Vec<i64> {
    // `advance`, `previous` and `skip_to_index` all bump `queue.version` without touching the row
    // set, so on a queue the size of the library every track change was building a `QueueRow` per
    // row, four `SharedString`s each, for the differ to find nothing. Same summaries in the same
    // order means no row's content can have moved and only the index has to be written.
    //
    // The row count is the other half and is not redundant: the close teardown empties the model
    // and deliberately leaves the shadow standing, that being what carries the selection across a
    // reopen. Without this term the reopen would match its own stale shadow and paint nothing.
    if queue_model.row_count() == qvm.queue_tracks.len()
        && same_sources(&shadow.lock(), &qvm.queue_tracks)
    {
        ui.global::<Queue>().set_current_index(qvm.queue_index);
        // **Still reports what the cache has no answer for**, which is what makes a failed fetch
        // retryable: it hands its claims back so "the next rebuild is the retry", and a track
        // change on an untouched queue is the commonest rebuild there is. Empty on every pass
        // after the fetch landed, the claim it takes standing in for the answer.
        return unresolved_ids(link_cache, &qvm.queue_tracks);
    }

    // Snapshot the old selection bits into a map so the per-row lookup
    // below is O(1) — a linear `.find()` per row made this O(n²) overall,
    // re-run on every queue mutation (including each frame of a drag).
    let old_sel: std::collections::HashMap<i64, bool> =
        shadow.lock().iter().map(|e| (e.id, e.selected)).collect();
    let mut new_shadow: Vec<ShadowEntry> = Vec::with_capacity(qvm.queue_tracks.len());
    let mut new_rows: Vec<QueueRow> = Vec::with_capacity(qvm.queue_tracks.len());
    {
        // One acquisition for the whole build rather than one per row. Nothing
        // inside awaits, and the only other writer is the fetch's own landing.
        let cached = link_cache.lock();
        for t in &qvm.queue_tracks {
            let selected = old_sel.get(&t.id).copied().unwrap_or(false);
            let mut row = to_slint_queue_row(t.as_ref(), selected);
            if let Some(links) = cached.get(&t.id).copied() {
                RowLinks::from(links).stamp(&mut row);
            }
            new_shadow.push(ShadowEntry {
                id: t.id,
                selected,
                source: Some(Arc::clone(t)),
            });
            new_rows.push(row);
        }
    }

    // The swap's own verdict rather than the shadow's, which only mirrors the model. It resets
    // when the rows moved, arrived or left and patches otherwise, so a `skip_to_index` that
    // bumps the queue version without touching the row set doesn't count as a change.
    let row_set_changed = crate::ui::model_diff::apply_rows_keyed(queue_model, new_rows, |r| r.id);
    super::write_selection(ui, &new_shadow);
    *shadow.lock() = new_shadow;
    let queue = ui.global::<Queue>();
    queue.set_current_index(qvm.queue_index);
    if row_set_changed {
        // Abort any in-flight drag-reorder: the row indices it was
        // computed against no longer describe the queue, and the model
        // reset destroys the row instance holding the pointer grab, so it
        // can never clear this state itself. Left set, the source row
        // stays ghosted and the drop line stranded.
        queue.set_drag_source(-1);
        queue.set_drop_slot(-1);
    }

    unresolved_ids(link_cache, &qvm.queue_tracks)
}

/// The track ids the link cache has no entry for, sorted and each once.
///
/// Asked by both of [`rebuild_rows`]' arms rather than collected while the rows are built: the
/// fast path builds none, and it is the path a failed fetch has to be retried from.
fn unresolved_ids(link_cache: &LinkCache, tracks: &[Arc<TrackSummary>]) -> Vec<i64> {
    let cached = link_cache.lock();
    let mut missing: Vec<i64> =
        tracks.iter().map(|t| t.id).filter(|id| !cached.contains_key(id)).collect();
    missing.sort_unstable();
    missing.dedup();
    missing
}

/// Whether `tracks` is the same set of summaries, in the same order, that the shadow was built
/// from.
///
/// Pointer equality rather than a field compare: `PlayerState` hands the same `Arc` back on a
/// reorder or an index move and only ever a fresh one where the row's content moved, so this
/// answers "nothing a row draws has changed" without reading a single field. A shadow whose
/// sources the teardown handed back answers `false` — see [`ShadowEntry`].
fn same_sources(shadow: &[ShadowEntry], tracks: &[Arc<TrackSummary>]) -> bool {
    shadow.len() == tracks.len()
        && shadow
            .iter()
            .zip(tracks)
            .all(|(seen, now)| seen.source.as_ref().is_some_and(|s| Arc::ptr_eq(s, now)))
}

/// Surgically flip `is_favorite` on every visible queue row whose `id` is in
/// `ids`. We do this rather than emitting via `sinks.queue` because
/// `with_state_emit` only publishes the queue VM when `queue.version` changed —
/// a favorite toggle isn't a queue mutation, so bumping the version would
/// trigger a full row rebuild for a single-bit change.
pub(crate) fn apply_row_favorite(weak: &Weak<AppWindow>, ids: &[i64], fav: bool) {
    let ids: HashSet<i64> = ids.iter().copied().collect();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        patch_rows_where(&ui.global::<Queue>().get_rows(), "queue favorite patch", |row| {
            let moved = ids.contains(&i64::from(row.id)) && row.is_favorite != fav;
            if moved {
                row.is_favorite = fav;
            }
            moved
        });
    });
}

/// Build a `QueueRow` from a `TrackSummary`. `pub(crate)` because the
/// full-screen Now Playing view's "Up Next" list (`ui::now_playing`)
/// reuses the exact same row shape — it always passes `selected: false`.
/// The row carries no cover; each surface resolves its own through its
/// `request-cover` callback, which is what lets the two share this
/// mapping while reading different `CoverThumbs` tiers.
///
/// The FK ids come out zeroed — Up Next has no context menu to need them, and
/// the queue sheet stamps its own once `links::fetch_missing` has answered.
pub(crate) fn to_slint_queue_row(t: &TrackSummary, selected: bool) -> QueueRow {
    let display_duration = format_duration_ms(t.duration_ms.max(0));
    QueueRow {
        id: i32::try_from(t.id).unwrap_or(i32::MAX),
        title: t.title.as_str().into(),
        artist: t.artist.as_deref().unwrap_or("").into(),
        artwork_path: t.artwork_path.as_deref().unwrap_or("").into(),
        display_duration: display_duration.into(),
        selected,
        is_favorite: t.is_favorite,
        album_id: 0,
        artist_id: 0,
        genre_id: 0,
    }
}

#[cfg(test)]
#[path = "tests/rows_tests.rs"]
mod tests;
