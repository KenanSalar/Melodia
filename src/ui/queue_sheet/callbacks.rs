//! `Queue.*` callback wiring + click-with-modifier selection helper.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, Model, VecModel};

use super::rows::rebuild_rows;
use super::{ShadowEntry, push_selected_count};
use crate::library;
use crate::media::cover_thumbs::CoverThumbs;
use crate::player::state::{PlayerAction, lock_state, with_state_emit};
use crate::state::AppState;
use crate::{AppWindow, Queue, QueueRow};

pub(super) fn wire_callbacks(
    ui: &AppWindow,
    state: &AppState,
    queue_model: &Rc<VecModel<QueueRow>>,
    queue_covers: &Arc<CoverThumbs>,
    shadow: &Arc<Mutex<Vec<ShadowEntry>>>,
    anchor: &Arc<Mutex<Option<usize>>>,
    is_open: &Arc<AtomicBool>,
) {
    let weak = ui.as_weak();
    let queue = ui.global::<Queue>();

    {
        let s = state.clone();
        queue.on_reorder(move |from, to| {
            let Ok(from) = usize::try_from(from) else { return };
            let Ok(to) = usize::try_from(to) else { return };
            if from == to {
                return;
            }
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_move(&s_inner, from, to) {
                    log::warn!("queue_move({from},{to}): {e}");
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_remove(move |idx| {
            let Ok(idx) = usize::try_from(idx) else { return };
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_remove(&s_inner, idx) {
                    log::warn!("queue_remove({idx}): {e}");
                }
            });
        });
    }

    {
        let s = state.clone();
        let shadow = shadow.clone();
        queue.on_remove_selected(move || {
            let indices: Vec<usize> = shadow
                .lock()
                .iter()
                .enumerate()
                .filter_map(|(i, e)| e.selected.then_some(i))
                .collect();
            if indices.is_empty() {
                return;
            }
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_remove_batch(&s_inner, &indices) {
                    log::warn!("queue_remove_batch({indices:?}): {e}");
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_skip_to(move |idx| {
            let Ok(idx) = usize::try_from(idx) else { return };
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_skip_to_index(&s_inner, idx) {
                    log::warn!("queue_skip_to_index({idx}): {e}");
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_clear_queue(move || {
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_clear(&s_inner) {
                    log::warn!("queue_clear: {e}");
                }
            });
        });
    }

    // Hover-swap favorite toggle from a queue row. Three updates:
    //   1. DB write via `library::favorites::set_favorite` (also bumps
    //      `library_changed_tx` so other tracklist views refetch).
    //   2. In-memory flip on `PlayerState.queue.tracks` (so the next
    //      sheet open / view-model emit reads the fresh value) and on
    //      `current_track` (so the Now Playing heart reflects it via
    //      the Player view-model push).
    //   3. Surgical patch of the visible `VecModel<QueueRow>` via
    //      `apply_row_favorite`. `with_state_emit` only publishes on
    //      `sinks.queue` when `queue.version` changes — a favorite
    //      toggle isn't a queue mutation, so the subscriber wouldn't
    //      fire and the row's `is_favorite` would stay stale (next
    //      click would compute `!false` again and be a no-op DB write).
    {
        let s = state.clone();
        let weak = weak.clone();
        queue.on_toggle_row_favorite(move |id, fav| {
            let id = i64::from(id);
            let s_inner = s.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::favorites::set_favorite(&s_inner, vec![id], fav).await {
                    log::warn!("queue toggle_row_favorite({id}, {fav}): {e}");
                    return;
                }
                with_state_emit(&s_inner.player_state, &s_inner.sinks, |st| {
                    for t in &mut st.queue.tracks {
                        if t.id == id {
                            Arc::make_mut(t).is_favorite = fav;
                        }
                    }
                    if let Some(ct) = st.current_track.as_mut()
                        && ct.id == id
                    {
                        Arc::make_mut(ct).is_favorite = fav;
                    }
                    Vec::<PlayerAction>::new()
                });
                super::rows::apply_row_favorite(&weak, id, fav);
            });
        });
    }

    // Reserved hook — the sheet's `Queue.close()` invocation lets us
    // attach future side effects without coupling to the open-changed
    // atomic. Slint also clears selection synchronously from the same
    // event handler, so this stays empty.
    queue.on_close(|| {});

    {
        let queue_model = queue_model.clone();
        let shadow = shadow.clone();
        let anchor = anchor.clone();
        let weak = weak.clone();
        queue.on_toggle_select(move |idx, ctrl, shift| {
            let Ok(idx) = usize::try_from(idx) else { return };
            apply_select(&queue_model, &shadow, &anchor, idx, ctrl, shift);
            push_selected_count(&weak, &shadow.lock());
        });
    }

    {
        let queue_model = queue_model.clone();
        let shadow = shadow.clone();
        let anchor = anchor.clone();
        let weak = weak.clone();
        queue.on_clear_selection(move || {
            let mut sh = shadow.lock();
            for e in sh.iter_mut() {
                e.selected = false;
            }
            // Mirror to the visible model so the accent border drops
            // synchronously instead of waiting for the next queue
            // broadcast.
            for i in 0..sh.len() {
                if let Some(mut row) = queue_model.row_data(i)
                    && row.selected
                {
                    row.selected = false;
                    queue_model.set_row_data(i, row);
                }
            }
            *anchor.lock() = None;
            push_selected_count(&weak, &sh);
        });
    }

    {
        let queue_model = queue_model.clone();
        let shadow = shadow.clone();
        let weak = weak.clone();
        queue.on_select_all(move || {
            let mut sh = shadow.lock();
            for e in sh.iter_mut() {
                e.selected = true;
            }
            for i in 0..sh.len() {
                if let Some(mut row) = queue_model.row_data(i)
                    && !row.selected
                {
                    row.selected = true;
                    queue_model.set_row_data(i, row);
                }
            }
            push_selected_count(&weak, &sh);
        });
    }

    {
        // Per-handler generation counter — every close bumps it and stamps
        // its own pending teardown with the post-increment value. The
        // teardown only fires when the *latest* close still owns the stamp,
        // so a stale 350 ms timer from an earlier close can never wipe rows
        // that a subsequent open→close has since rebuilt. Combined with the
        // is_open re-check inside the UI closure (the only true
        // serialization point with the open callback), this closes both
        // races: (1) TOCTOU between the tokio-side `is_open` check and the
        // UI-thread wipe, and (2) older-close timer firing during a newer
        // close's slide-out.
        let close_epoch = Arc::new(AtomicU64::new(0));
        let is_open = is_open.clone();
        let weak = weak.clone();
        let state = state.clone();
        let queue_model = queue_model.clone();
        let queue_covers = queue_covers.clone();
        let shadow = shadow.clone();
        queue.on_open_changed(move |open| {
            is_open.store(open, Ordering::Relaxed);
            if open {
                // Open is a two-phase render:
                //
                // 1. **Skeleton (synchronous, this thread):** build rows
                //    from the current `PlayerState` using a *cache-only*
                //    cover lookup. Rows land in the model before the
                //    callback returns, so the slide-up animation has
                //    title / artist / duration / current-row accent from
                //    frame one.
                //
                // 2. **Warmup (off-thread):** decode the missing covers
                //    in parallel on the cover-decode Rayon pool.
                //
                // 3. **Rebuild (UI thread again):** once the warmup
                //    future resolves, re-read `PlayerState` and rebuild
                //    rows with the decoding lookup — by now every cover
                //    is a cache hit, so the covers fade in.
                let Some(ui) = weak.upgrade() else { return };
                let qvm = {
                    let s = lock_state(&state.player_state);
                    s.to_queue_view_model()
                };
                // Step 1: skeleton (cache-only lookup, no decode).
                rebuild_rows(&ui, &queue_model, &shadow, &qvm, |p| {
                    queue_covers.get_cached_opt(p)
                });
                drop(ui);
                let paths: Vec<PathBuf> = qvm
                    .queue_tracks
                    .iter()
                    .filter_map(|t| {
                        t.artwork_path
                            .as_deref()
                            .filter(|p| !p.is_empty())
                            .map(PathBuf::from)
                    })
                    .collect();
                drop(qvm);
                let weak = weak.clone();
                let queue_covers = queue_covers.clone();
                let shadow = shadow.clone();
                let is_open = is_open.clone();
                let state = state.clone();
                // `queue_model: Rc<…>` isn't `Send`, so the async block
                // re-acquires the `VecModel` on the UI side via the same
                // `downcast_ref` pattern the close branch uses.
                state.runtime.clone().spawn(async move {
                    // Step 2: warm covers off the UI thread.
                    if !paths.is_empty() {
                        let covers = queue_covers.clone();
                        let _ = tokio::task::spawn_blocking(move || covers.prewarm(&paths))
                            .await;
                    }
                    // Step 3: full rebuild on the UI thread (cache hits).
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        if !is_open.load(Ordering::Relaxed) {
                            return; // closed during warmup
                        }
                        let rows = ui.global::<Queue>().get_rows();
                        let Some(vm) =
                            rows.as_any().downcast_ref::<VecModel<QueueRow>>()
                        else {
                            return;
                        };
                        let qvm = {
                            let s = lock_state(&state.player_state);
                            s.to_queue_view_model()
                        };
                        rebuild_rows(&ui, vm, &shadow, &qvm, |p| {
                            queue_covers.get_or_load_opt(p)
                        });
                    });
                });
            } else {
                // Closing — once the slide-out animation finishes, drop the
                // row model + decoded covers and hand the freed pages back
                // to the OS.
                let my_epoch = close_epoch.fetch_add(1, Ordering::AcqRel) + 1;
                let is_open = is_open.clone();
                let weak = weak.clone();
                let queue_covers = queue_covers.clone();
                let close_epoch = close_epoch.clone();
                let runtime = state.runtime.clone();
                runtime.clone().spawn(async move {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    if is_open.load(Ordering::Relaxed)
                        || close_epoch.load(Ordering::Acquire) != my_epoch
                    {
                        return; // reopened, or a newer close superseded us
                    }
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        // Final gate — the UI thread is the serialization
                        // point with `on_open_changed(true)`.
                        if is_open.load(Ordering::Relaxed)
                            || close_epoch.load(Ordering::Acquire) != my_epoch
                        {
                            return;
                        }
                        // Drop the visible rows — this releases the
                        // `Image` handles on each row's `cover_img`.
                        if let Some(vm) = ui
                            .global::<Queue>()
                            .get_rows()
                            .as_any()
                            .downcast_ref::<VecModel<QueueRow>>()
                        {
                            vm.set_vec(Vec::new());
                        }
                        // Drop the cache's buffer refs too — only with
                        // both gone is the underlying memory actually
                        // freed.
                        queue_covers.clear();
                        runtime.spawn_blocking(crate::tasks::heap_trim::trim);
                    });
                });
            }
        });
    }
}

/// Apply a click with optional Ctrl/Shift to the selection. Updates
/// the shadow and mirrors the change into the live `VecModel<QueueRow>`
/// so the accent border flips synchronously. Anchor follows plain
/// and Ctrl clicks; Shift extends from the current anchor without
/// moving it.
fn apply_select(
    queue_model: &Rc<VecModel<QueueRow>>,
    shadow: &Arc<Mutex<Vec<ShadowEntry>>>,
    anchor: &Arc<Mutex<Option<usize>>>,
    idx: usize,
    ctrl: bool,
    shift: bool,
) {
    let mut sh = shadow.lock();
    if idx >= sh.len() {
        return;
    }
    let mut a = anchor.lock();

    if shift && let Some(anchor_idx) = *a {
        let from = anchor_idx.min(idx);
        let to = anchor_idx.max(idx);
        for (i, e) in sh.iter_mut().enumerate() {
            e.selected = i >= from && i <= to;
        }
    } else if ctrl {
        sh[idx].selected = !sh[idx].selected;
        *a = Some(idx);
    } else {
        for (i, e) in sh.iter_mut().enumerate() {
            e.selected = i == idx;
        }
        *a = Some(idx);
    }

    for (i, e) in sh.iter().enumerate() {
        if let Some(mut row) = queue_model.row_data(i)
            && row.selected != e.selected
        {
            row.selected = e.selected;
            queue_model.set_row_data(i, row);
        }
    }
}
