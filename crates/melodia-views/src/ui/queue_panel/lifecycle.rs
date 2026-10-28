//! The queue panel's lifecycle: the toggle, the queue shown as the panel comes on screen, and its
//! rows and covers handed back once it has gone.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_compat::Compat;
use slint::{ComponentHandle, Weak};

use super::rows::{read_queue, show_queue};
use crate::ui::nav_transition;
use crate::ui::paged_rows::queue_rows::{self, QueueRows};
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Nav, NavEnterFrom, Queue};

/// How long a hidden panel keeps its rows and covers: past its ease out, so they don't vanish
/// from a panel still on screen.
const TEARDOWN_DELAY: Duration = Duration::from_millis(350);

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    rows: &Rc<QueueRows>,
    queue_covers: &Arc<CoverThumbs>,
    shown: &Arc<AtomicBool>,
) {
    wire_toggle(ui);
    wire_shown(ui, state, rows, queue_covers, shown);
}

/// The bar's button, its overflow entry and Q. Under Now Playing the panel is open but stepped
/// aside, so a press closes Now Playing to bring it back beside the library rather than closing
/// the queue nobody can see. A window with no room to dock it opens the overlay instead.
fn wire_toggle(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Queue>().on_toggle(move || {
        let Some(ui) = weak.upgrade() else { return };
        let queue = ui.global::<Queue>();
        if queue.get_shown() {
            queue.invoke_close();
            return;
        }
        let nav = ui.global::<Nav>();
        if nav.get_now_playing_open() {
            nav_transition::mark(&ui, NavEnterFrom::Above);
            nav.set_now_playing_open(false);
        }
        if queue.get_fits() {
            queue.set_open(true);
        } else {
            queue.set_overlay_open(true);
        }
    });
}

/// Show the queue as the panel comes on screen, hand everything back once it has been gone a
/// while. Hidden covers a close, Now Playing, the miniplayer and a narrow window alike.
fn wire_shown(
    ui: &AppWindow,
    state: &AppState,
    rows: &Rc<QueueRows>,
    queue_covers: &Arc<CoverThumbs>,
    shown: &Arc<AtomicBool>,
) {
    // Each hide stamps its teardown with a new epoch and only the latest one's fires, so an
    // earlier hide's timer can't wipe rows a later show seated. The `shown` re-check on the UI
    // thread closes the gap between the worker's check and the wipe.
    let hide_epoch = Arc::new(AtomicU64::new(0));
    let showing = Showing {
        weak: ui.as_weak(),
        state: state.clone(),
        rows: rows.clone(),
        queue_covers: queue_covers.clone(),
        shown: shown.clone(),
    };
    ui.global::<Queue>().on_shown_changed(move |shown| {
        log::debug!("queue panel: {}", if shown { "shown" } else { "hidden" });
        showing.shown.store(shown, Ordering::Relaxed);
        if shown {
            showing.show();
        } else {
            showing.schedule_teardown(&hide_epoch);
        }
    });
}

/// What the show and the hide reach.
struct Showing {
    weak: Weak<AppWindow>,
    state: AppState,
    rows: Rc<QueueRows>,
    queue_covers: Arc<CoverThumbs>,
    shown: Arc<AtomicBool>,
}

impl Showing {
    /// Show the queue in two steps:
    ///
    /// 1. **The ids, on this thread**, so the panel has the queue's length and its current row
    ///    from frame one, its rows reading as placeholders until text lands. Each row asks for its
    ///    own cover, which borrows from the row tier or is scheduled, so the rows the `ListView`
    ///    builds on that frame can't drag a decode onto the UI thread.
    /// 2. **The rows around the current one**, fetched off-thread and shown as they land, the
    ///    screenful the panel seats itself on.
    fn show(&self) {
        let Some(ui) = self.weak.upgrade() else { return };
        let read = read_queue(&self.state, &self.rows);
        let (ids, at) = (Arc::clone(&read.ids), usize::try_from(read.index).unwrap_or(0));
        show_queue(&ui, &self.rows, read);
        drop(ui);

        let seeded = {
            let state = self.state.clone();
            self.state.runtime.spawn(async move { queue_rows::seed(&state, ids, at).await })
        };
        let (rows, shown) = (self.rows.clone(), self.shown.clone());
        let spawned = slint::spawn_local(Compat::new(async move {
            let keyset = match seeded.await {
                Ok(Ok(keyset)) => keyset,
                Ok(Err(e)) => {
                    log::warn!("queue panel: seed fetch failed: {}", describe(&e));
                    return;
                }
                Err(e) => {
                    log::warn!("queue panel: seed task failed: {}", describe(&e));
                    return;
                }
            };
            if shown.load(Ordering::Relaxed) {
                rows.land_seed(keyset);
            }
        }));
        if let Err(e) = spawned {
            log::warn!("queue panel: show not spawned: {}", describe(&e));
        }
    }

    /// Once the panel has stayed hidden past the delay, drop the rows and decoded covers and hand
    /// the freed pages back to the OS. The picks stay, as on every other list.
    fn schedule_teardown(&self, hide_epoch: &Arc<AtomicU64>) {
        let my_epoch = hide_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let shown = self.shown.clone();
        let weak = self.weak.clone();
        let queue_covers = self.queue_covers.clone();
        let hide_epoch = hide_epoch.clone();
        let runtime = self.state.runtime.clone();
        runtime.clone().spawn(async move {
            tokio::time::sleep(TEARDOWN_DELAY).await;
            if shown.load(Ordering::Relaxed) || hide_epoch.load(Ordering::Acquire) != my_epoch {
                return;
            }
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if shown.load(Ordering::Relaxed) || hide_epoch.load(Ordering::Acquire) != my_epoch {
                    return;
                }
                let rows = ui.global::<Queue>().get_rows();
                if let Some(rows) = QueueRows::installed(&rows, "queue panel teardown") {
                    rows.clear();
                }
                // Drop the cache's buffer refs too. A cover the panel decoded is freed once both
                // are gone; one it borrowed stays with the row tier.
                queue_covers.clear();
                runtime.spawn_blocking(melodia_platform::services::platform::allocator::trim);
            });
        });
    }
}
