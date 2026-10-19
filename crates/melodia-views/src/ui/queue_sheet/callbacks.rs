//! `Queue.*` callback wiring: the transport rows, the heart and the stars, the picks, and the
//! sheet's open and close.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_compat::Compat;
use slint::{ComponentHandle, Weak};

use super::rows::show_queue;
use super::{apply_picks, picked, picked_positions};
use crate::ui::callbacks::collect_track_ids;
use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::grid_prewarm::unique_artwork_paths;
use crate::ui::list_selection::{NO_ANCHOR, compute_click_selection};
use crate::ui::paged_rows::Keyset;
use crate::ui::paged_rows::queue_rows::{self, QueueRows};
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::error::describe;
use melodia_engine::player::engine::state::lock_state;
use melodia_ui::{AppWindow, Queue, QueueRow};

/// Covers decoded ahead of first paint when the sheet opens.
///
/// Rows resolve their own covers lazily, so this only has to cover what
/// the slide-up actually reveals; everything scrolled to afterwards
/// decodes on demand, the same way the track lists behave past their own
/// prewarmed prefix.
const QUEUE_PREWARM_AHEAD: usize = 24;

/// How long the slide-out runs before the teardown hands the rows and covers back.
const TEARDOWN_DELAY: Duration = Duration::from_millis(350);

pub(super) fn wire_callbacks(
    ui: &AppWindow,
    state: &AppState,
    rows: &Rc<QueueRows>,
    queue_covers: &Arc<CoverThumbs>,
    anchor: &Rc<Cell<i32>>,
    is_open: &Arc<AtomicBool>,
) {
    wire_transport(ui, state, rows);
    wire_row_flags(ui, state);
    wire_selection(ui, rows, anchor);
    wire_open_close(ui, state, rows, queue_covers, anchor, is_open);
}

/// The five rows that only ask the engine for something, plus the sheet's reserved close hook.
///
/// None of them reports anything back: the mutation lands on `sinks.queue` and the subscriber
/// shows it from there.
fn wire_transport(ui: &AppWindow, state: &AppState, rows: &Rc<QueueRows>) {
    let queue = ui.global::<Queue>();

    {
        let s = state.clone();
        let rows = rows.clone();
        queue.on_reorder(move |from, to| {
            let Ok(from) = usize::try_from(from) else {
                return;
            };
            let Ok(to) = usize::try_from(to) else { return };
            if from == to {
                return;
            }
            // Moved here first, so the queue the engine publishes back is the one already on
            // show and every other row keeps its delegate.
            rows.move_row(from, to);
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_move(&s_inner, from, to) {
                    log::warn!("queue_move({from},{to}): {}", describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_remove(move |idx| {
            let Ok(idx) = usize::try_from(idx) else {
                return;
            };
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_remove(&s_inner, idx) {
                    log::warn!("queue_remove({idx}): {}", describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        let rows = rows.clone();
        let weak = ui.as_weak();
        queue.on_remove_selected(move || {
            let Some(ui) = weak.upgrade() else { return };
            let indices = picked_positions(&rows, &picked(&ui));
            if indices.is_empty() {
                return;
            }
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_remove_batch(&s_inner, &indices) {
                    log::warn!("queue_remove_batch({} rows): {}", indices.len(), describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        queue.on_skip_to(move |idx| {
            let Ok(idx) = usize::try_from(idx) else {
                return;
            };
            let s_inner = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::queue::queue_skip_to_index(&s_inner, idx).await {
                    log::warn!("queue_skip_to_index({idx}): {}", describe(&e));
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
                    log::warn!("queue_clear: {}", describe(&e));
                }
            });
        });
    }

    // Reserved hook — the sheet's `Queue.close()` invocation lets us
    // attach future side effects without coupling to the open-changed
    // atomic. Slint also clears selection synchronously from the same
    // event handler, so this stays empty.
    queue.on_close(|| {});
}

/// The heart and the star strip, from a row or from the menu: the write, then the rows it covers
/// patched, as on every track list.
///
/// Each write reaches the playing track and bumps `library_changed`, which re-reads every row the
/// sheet holds and the records the player starts from. The patch is what moves the row now rather
/// than a fetch later, and the heart's is also what its next click reads, which would otherwise
/// compute `!false` again and write nothing.
fn wire_row_flags(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Queue>();
    let weak = ui.as_weak();
    wire_row_flag!(g, on_toggle_row_favorite, state, "queue::set_favorite",
    library::favorites::set_favorite, collect_track_ids,
    captures: [weak],
    after: |id_vec, fav| {
        patch_rows(&weak, id_vec, move |row| row.is_favorite = fav);
    });
    wire_row_flag!(g, on_set_row_rating, state, "queue::set_rating",
    library::ratings::set_rating, collect_track_ids,
    captures: [weak],
    after: |id_vec, rating| {
        patch_rows(&weak, id_vec, move |row| row.rating = rating);
    });
}

/// Apply `edit` to the cached rows of `ids`, every copy of a track queued twice included.
fn patch_rows(
    weak: &Weak<AppWindow>,
    ids: Vec<i64>,
    edit: impl Fn(&mut QueueRow) + Send + 'static,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let model = ui.global::<Queue>().get_rows();
        let Some(rows) = QueueRows::installed(&model, "queue row patch") else {
            return;
        };
        for id in ids {
            rows.patch(id, &edit);
        }
    });
}

/// The three selection rows, each over the shared click semantics the track lists use.
fn wire_selection(ui: &AppWindow, rows: &Rc<QueueRows>, anchor: &Rc<Cell<i32>>) {
    let weak = ui.as_weak();
    let queue = ui.global::<Queue>();

    {
        let (rows, anchor, weak) = (rows.clone(), anchor.clone(), weak.clone());
        queue.on_toggle_select(move |idx, ctrl, shift| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(id) = usize::try_from(idx).ok().and_then(|at| rows.ids().get(at).copied())
            else {
                return;
            };
            let (picks, next_anchor) = compute_click_selection(
                anchor.get(),
                picked(&ui).into_iter().collect(),
                || rows.row_ids(),
                idx,
                clamp_i64_to_i32(id),
                shift,
                ctrl,
            );
            anchor.set(next_anchor);
            apply_picks(&ui, &rows, &picks.into_iter().collect());
        });
    }

    {
        let (rows, anchor, weak) = (rows.clone(), anchor.clone(), weak.clone());
        queue.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            apply_picks(&ui, &rows, &HashSet::new());
            // Only the clear drops the anchor: Select All leaves whatever a click last established,
            // so a following Shift-click still extends from where the user was.
            anchor.set(NO_ANCHOR);
        });
    }

    {
        let rows = rows.clone();
        queue.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            apply_picks(&ui, &rows, &rows.row_ids().into_iter().collect());
        });
    }
}

/// The sheet's whole lifecycle: show the queue on the way up, hand everything back once the
/// slide-out has finished.
fn wire_open_close(
    ui: &AppWindow,
    state: &AppState,
    rows: &Rc<QueueRows>,
    queue_covers: &Arc<CoverThumbs>,
    anchor: &Rc<Cell<i32>>,
    is_open: &Arc<AtomicBool>,
) {
    // Per-handler generation counter — every close bumps it and stamps
    // its own pending teardown with the post-increment value. The
    // teardown only fires when the *latest* close still owns the stamp,
    // so a stale teardown timer from an earlier close can never wipe rows
    // that a subsequent open→close has since shown. Combined with the
    // is_open re-check inside the UI closure (the only true
    // serialization point with the open callback), this closes both
    // races: (1) TOCTOU between the tokio-side `is_open` check and the
    // UI-thread wipe, and (2) older-close timer firing during a newer
    // close's slide-out.
    let close_epoch = Arc::new(AtomicU64::new(0));
    let opening = Opening {
        weak: ui.as_weak(),
        state: state.clone(),
        rows: rows.clone(),
        anchor: anchor.clone(),
        queue_covers: queue_covers.clone(),
        is_open: is_open.clone(),
    };
    ui.global::<Queue>().on_open_changed(move |open| {
        log::debug!("queue sheet: {}", if open { "open" } else { "closed" });
        opening.is_open.store(open, Ordering::Relaxed);
        if open {
            opening.open();
        } else {
            opening.schedule_teardown(&close_epoch);
        }
    });
}

/// What the open and the close reach.
struct Opening {
    weak: Weak<AppWindow>,
    state: AppState,
    rows: Rc<QueueRows>,
    anchor: Rc<Cell<i32>>,
    queue_covers: Arc<CoverThumbs>,
    is_open: Arc<AtomicBool>,
}

impl Opening {
    /// Show the queue in three steps:
    ///
    /// 1. **The ids, on this thread**, so the slide-up has the queue's length and its current row
    ///    from frame one, its rows reading as placeholders until text lands. `covers-generation`
    ///    is still 0 here, which holds `Queue.request-cover` to a cache-only lookup, so the rows
    ///    the `ListView` instantiates on that first frame can't drag a decode onto the UI thread.
    ///    They still paint every cover the row tier holds.
    /// 2. **The rows around the current one**, fetched off-thread and shown as they land, the
    ///    screenful the sheet seats itself on.
    /// 3. **That screenful's covers warmed, then `covers-generation` bumped**: each row pulls its
    ///    own cover through `Queue.request-cover`, and a `pure` callback's result is cached until
    ///    a dependency is dirtied, so the bump both re-runs the lookup and switches it to the
    ///    decoding one.
    fn open(&self) {
        let Some(ui) = self.weak.upgrade() else { return };
        let qvm = lock_state(&self.state.player_state).to_queue_view_model();
        show_queue(&ui, &self.rows, &self.anchor, &qvm);
        drop(ui);

        let at = usize::try_from(qvm.queue_index).unwrap_or(0);
        let seeded = {
            let state = self.state.clone();
            self.state.runtime.spawn(async move { queue_rows::seed(&state, qvm.ids, at).await })
        };
        let (weak, rows) = (self.weak.clone(), self.rows.clone());
        let (queue_covers, is_open) = (self.queue_covers.clone(), self.is_open.clone());
        let spawned = slint::spawn_local(Compat::new(async move {
            let keyset = match seeded.await {
                Ok(Ok(keyset)) => Some(keyset),
                Ok(Err(e)) => {
                    log::warn!("queue sheet: seed fetch failed: {}", describe(&e));
                    None
                }
                Err(e) => {
                    log::warn!("queue sheet: seed task failed: {}", describe(&e));
                    None
                }
            };
            if !is_open.load(Ordering::Relaxed) {
                return; // closed while it fetched
            }
            let paths = keyset.as_ref().map(|keyset| prewarm_paths(keyset, at)).unwrap_or_default();
            // Only over the ids it was fetched for: a queue that moved since is already on show
            // and fetching its own rows.
            if let Some(keyset) = keyset.filter(|keyset| *keyset.ids == *rows.ids()) {
                rows.show(keyset);
            }
            // No early return on an empty `paths`, even though there is then nothing to decode:
            // the bump is also what takes the lookup off `get_cached_opt`, so skipping it would
            // park a queue whose tracks all lack artwork at generation 0 for the whole open
            // session, and rows added afterwards would show a placeholder that never resolves.
            let covers = queue_covers.clone();
            let _ = tokio::task::spawn_blocking(move || covers.prewarm(&paths)).await;
            if !is_open.load(Ordering::Relaxed) {
                return; // closed during warmup
            }
            if let Some(ui) = weak.upgrade() {
                let queue = ui.global::<Queue>();
                queue.set_covers_generation(queue.get_covers_generation().wrapping_add(1));
            }
        }));
        if let Err(e) = spawned {
            log::warn!("queue sheet: open not spawned: {}", describe(&e));
        }
    }

    /// Once the slide-out has finished, drop the rows and decoded covers and hand the freed pages
    /// back to the OS. The picks stay, as on every other list.
    fn schedule_teardown(&self, close_epoch: &Arc<AtomicU64>) {
        let my_epoch = close_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let is_open = self.is_open.clone();
        let weak = self.weak.clone();
        let queue_covers = self.queue_covers.clone();
        let close_epoch = close_epoch.clone();
        let runtime = self.state.runtime.clone();
        runtime.clone().spawn(async move {
            tokio::time::sleep(TEARDOWN_DELAY).await;
            if is_open.load(Ordering::Relaxed) || close_epoch.load(Ordering::Acquire) != my_epoch {
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
                let queue = ui.global::<Queue>();
                if let Some(rows) = QueueRows::installed(&queue.get_rows(), "queue sheet teardown")
                {
                    rows.clear();
                }
                // Drop the cache's buffer refs too. A cover the
                // sheet decoded is freed once both are gone; one it
                // borrowed stays with the row tier. Rewinding the
                // generation alongside it is what keeps 0 meaning
                // "this tier is cold" rather than "first open of
                // the session": the next open faces the same empty
                // cache, so it owes the same cache-only first frame.
                queue.set_covers_generation(0);
                queue_covers.clear();
                runtime.spawn_blocking(melodia_platform::services::platform::allocator::trim);
            });
        });
    }
}

/// The covers of the seed's rows from the current one on, which is what the sheet seats on screen.
fn prewarm_paths(keyset: &Keyset<QueueRow>, at: usize) -> Vec<PathBuf> {
    let current = keyset.ids.get(at).copied();
    let from = keyset.seed.iter().position(|(id, _)| Some(*id) == current).unwrap_or(0);
    unique_artwork_paths(
        keyset.seed[from..].iter().map(|(_, row)| Some(row.artwork_path.as_str())),
        QUEUE_PREWARM_AHEAD,
    )
}
