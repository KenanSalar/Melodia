//! The queue's lifecycle: shown on the way in, its rows and covers handed back once it has gone.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_compat::Compat;
use slint::{ComponentHandle, Weak};

use super::rows::show_queue;
use crate::ui::grid_prewarm::unique_artwork_paths;
use crate::ui::paged_rows::Keyset;
use crate::ui::paged_rows::queue_rows::{self, QueueRows};
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

/// The sheet's whole lifecycle: show the queue on the way up, hand everything back once the
/// slide-out has finished.
pub(super) fn wire(
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
