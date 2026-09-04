//! OS file-drop batching for the queue-sheet drop target.
//!
//! Winit fires `WindowEvent::DroppedFile` once *per file*, so a five-track drag lands as
//! five events in one event-loop iteration. Uncoalesced that is five concurrent
//! `queue_import_files` tasks racing for the same write transaction and serialising
//! anyway, publishing five intermediate queue snapshots the UI re-renders for nothing.
//!
//! So the first event in a batch wins `flush_scheduled` and spawns one task; the rest
//! push their path and piggyback. That task sleeps — generously, the real cluster being
//! microseconds — then drains `pending`, clears the flag and submits the whole batch.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use melodia_app::library;
use melodia_app::state::AppState;

/// Atomic mirror of `Queue.open`, which the `DroppedFile` handler consults to decide
/// whether the user is targeting the queue sheet. A `OnceLock` so the queue sheet can
/// install its callbacks *after* `window_chrome::install` registered the winit filter —
/// the handler reads through it at event-fire time, so a late set still works.
static QUEUE_SHEET_OPEN: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Forward the `Queue.open` atomic so the `DroppedFile` filter can route into the queue.
/// Idempotent: a second call is a silent no-op, the sheet installing once per process.
pub fn set_queue_sheet_open(handle: Arc<AtomicBool>) {
    let _ = QUEUE_SHEET_OPEN.set(handle);
}

/// Read-only view of the flag, `false` before the sheet has installed its atomic. Used
/// by the opt-in RSS sampler to annotate log lines with the overlay state.
pub fn is_queue_sheet_open() -> bool {
    QUEUE_SHEET_OPEN.get().is_some_and(|atomic| atomic.load(Ordering::Relaxed))
}

/// Currently-open playlist id, mirrored from `PlaylistDetail.playlist-id`; `-1` when the
/// detail is closed. The flush reads it at fire time, routing there when no queue sheet
/// takes precedence. One `AtomicI64` rather than a second `AtomicBool` gate — the `>= 0`
/// test answers both "is a detail open" and "which playlist".
static CURRENT_PLAYLIST_ID: AtomicI64 = AtomicI64::new(-1);

/// Update the open playlist id: set on `open_playlist`, cleared on `close_detail`.
pub fn set_current_playlist_id(id: i64) {
    CURRENT_PLAYLIST_ID.store(id, Ordering::Release);
}

/// Read-only view of "is a playlist detail open", for the opt-in RSS sampler.
pub fn is_playlist_detail_open() -> bool {
    CURRENT_PLAYLIST_ID.load(Ordering::Relaxed) >= 0
}

struct DropCoalescer {
    pending: Mutex<Vec<PathBuf>>,
    flush_scheduled: AtomicBool,
}

static DROP_COALESCER: OnceLock<DropCoalescer> = OnceLock::new();

fn drop_coalescer() -> &'static DropCoalescer {
    DROP_COALESCER.get_or_init(|| DropCoalescer {
        pending: Mutex::new(Vec::new()),
        flush_scheduled: AtomicBool::new(false),
    })
}

/// Push a dropped path into the coalescer, scheduling a single delayed flush if one
/// isn't in flight. That flush re-checks the routing at fire time, so a drop that
/// started while the sheet was open and landed after it closed is discarded.
pub(super) fn schedule_drop_flush(state: &AppState, path: PathBuf) {
    let coalescer = drop_coalescer();
    coalescer.pending.lock().push(path);

    // Only the first event spawns; later pushes piggyback. `AcqRel` so the `push` above
    // is published to the flush task before it observes `flush_scheduled`.
    if coalescer
        .flush_scheduled
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let state = state.clone();
    state.runtime.clone().spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let coalescer = drop_coalescer();
        let batch = std::mem::take(&mut *coalescer.pending.lock());
        coalescer.flush_scheduled.store(false, Ordering::Release);

        if batch.is_empty() {
            return;
        }

        // Precedence, re-checked *after* the wait so a drop that started with one
        // overlay open and landed after a navigate-away routes to the new target: the
        // queue sheet first, sitting on top of every view; else an open playlist detail;
        // else discard, no current target accepting files.
        let queue_open =
            QUEUE_SHEET_OPEN.get().is_some_and(|atomic| atomic.load(Ordering::Relaxed));
        let playlist_open = CURRENT_PLAYLIST_ID.load(Ordering::Relaxed) >= 0;

        let paths: Vec<String> = batch.iter().map(|p| p.to_string_lossy().into_owned()).collect();

        if queue_open {
            match library::queue::queue_import_files(&state, paths).await {
                Ok(_) => {
                    // The queue itself updates through `sinks.queue`, but no other
                    // view sees freshly-imported tracks without this bump.
                    state.library_changed.bump();
                }
                Err(e) => {
                    log::warn!("queue_import_files (drop): {e}");
                    melodia_core::utils::toast::notify(
                        melodia_core::utils::toast::ToastKind::OperationFailed,
                        e.to_string(),
                    );
                }
            }
            return;
        }

        if playlist_open {
            let id = CURRENT_PLAYLIST_ID.load(Ordering::Acquire);
            if id >= 0 {
                match library::playlists::import_files_to_playlist(&state, id, paths).await {
                    Ok(_) => {
                        // Refreshes the open detail's track list *and* the grid — a
                        // previously-empty playlist has a new count and thumbnail.
                        state.library_changed.bump();
                    }
                    Err(e) => log::warn!("import_files_to_playlist (drop): {e}"),
                }
            }
        }
    });
}
