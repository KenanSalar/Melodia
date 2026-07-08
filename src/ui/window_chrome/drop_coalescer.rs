//! OS file-drop batching for the queue-sheet drop target.
//!
//! Winit fires `WindowEvent::DroppedFile` once *per file*. Selecting
//! five tracks in Dolphin and dragging them lands as five distinct
//! events delivered within the same event-loop iteration. Without
//! coalescing, we'd spawn five concurrent `queue_import_files` tasks,
//! each racing for the same `SQLx` write transaction and serialising
//! against each other anyway — slow, racy, and the `add_tracks` calls
//! publish five intermediate queue snapshots that the UI re-renders
//! for nothing.
//!
//! Strategy: the first `DroppedFile` in a batch wins the
//! `flush_scheduled` flag and spawns a single coalesced task. That
//! task sleeps 50 ms (generous; real cluster is microseconds), drains
//! `pending`, clears the flag, and submits the whole batch as one
//! `queue_import_files` call. Subsequent `DroppedFile`s in the same
//! batch just push their path and observe `flush_scheduled = true`,
//! piggybacking on the in-flight task.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::library;
use crate::state::AppState;

/// Atomic mirror of `Queue.open` published by
/// `crate::ui::queue_sheet::install`. The winit `WindowEvent::DroppedFile`
/// handler consults it to decide whether the user is currently
/// targeting the queue sheet with a drop. We use a `OnceLock` so the
/// queue sheet can install its callbacks *after* `window_chrome::install`
/// has registered the winit filter — the handler reads through the
/// lock at event-fire time, not at install time, so a late
/// `set_queue_sheet_open` call still works.
static QUEUE_SHEET_OPEN: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Forward the `Queue.open` atomic so the winit `DroppedFile` filter
/// can route dropped paths into the queue when the sheet is open.
/// Idempotent: a second call after `set` is a silent no-op (the
/// queue sheet only installs once per process anyway).
pub fn set_queue_sheet_open(handle: Arc<AtomicBool>) {
    let _ = QUEUE_SHEET_OPEN.set(handle);
}

/// Read-only view of the queue-sheet open flag. Returns `false` if the
/// queue sheet hasn't installed its atomic yet (early-boot state). Used by
/// the opt-in RSS sampler to annotate log lines with the current overlay
/// state.
pub fn is_queue_sheet_open() -> bool {
    QUEUE_SHEET_OPEN
        .get()
        .is_some_and(|atomic| atomic.load(Ordering::Relaxed))
}

/// Currently-open playlist id, mirrored from
/// `PlaylistDetail.playlist-id` via [`set_current_playlist_id`].
/// `-1` ⇒ no playlist open / detail closed. The drop flush reads this
/// at fire time: if a queue sheet isn't taking precedence and this is
/// `>= 0`, dropped files route into that playlist.
///
/// We use a single `AtomicI64` (not an extra `AtomicBool` gate) — the
/// `>= 0` test serves as both "is a detail open" and "which playlist".
static CURRENT_PLAYLIST_ID: AtomicI64 = AtomicI64::new(-1);

/// Update the current open playlist id. Set on `open_playlist`; cleared
/// (`-1`) on `close_detail`. The drop flush task reads this at fire time.
pub fn set_current_playlist_id(id: i64) {
    CURRENT_PLAYLIST_ID.store(id, Ordering::Release);
}

/// Read-only view of "is a playlist detail open" — used by the opt-in
/// RSS sampler to annotate log lines with the active overlay state.
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

/// Push a dropped path into the coalescer, scheduling a single
/// 50 ms-delayed flush if one isn't already in flight. The flush
/// re-checks `QUEUE_SHEET_OPEN` at fire time so a drop that started
/// while the sheet was open but landed after it closed is correctly
/// discarded — and vice versa.
pub(super) fn schedule_drop_flush(state: &AppState, path: PathBuf) {
    let coalescer = drop_coalescer();
    coalescer.pending.lock().push(path);

    // Only the first event in a batch spawns the flush task; later
    // pushes piggyback. `AcqRel` because we want to publish the
    // `pending.push` above to the flush task before it observes
    // `flush_scheduled == true`.
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

        // Routing precedence at flush time (re-checked AFTER the 50 ms
        // wait so a drop that started with one overlay open but landed
        // after a navigate-away routes to the new target):
        //   1. Queue Sheet open ⇒ append to queue (most immediate, the
        //      sheet sits on top of every view).
        //   2. Else if a Playlist Detail is open ⇒ import + add to it.
        //   3. Else discard — no current drop target accepts files.
        let queue_open = QUEUE_SHEET_OPEN
            .get()
            .is_some_and(|atomic| atomic.load(Ordering::Relaxed));
        let playlist_open = CURRENT_PLAYLIST_ID.load(Ordering::Relaxed) >= 0;

        let paths: Vec<String> = batch
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        if queue_open {
            match library::queue::queue_import_files(&state, paths).await {
                Ok(_) => {
                    // Bump the library-changed counter so every view's
                    // subscriber (Tracks / Albums / Artists / Genres /
                    // Playlists) re-fetches. The queue itself already
                    // updates via `sinks.queue`, but other views won't
                    // see freshly-imported tracks without this nudge.
                    state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));
                }
                Err(e) => {
                    log::warn!("queue_import_files (drop): {e}");
                    crate::services::toast::notify(
                        crate::services::toast::ToastKind::OperationFailed,
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
                        // Bump library-changed so the playlist's
                        // detail subscriber (`wire_playlists::library_changed`)
                        // refreshes the open detail's track list AND the
                        // grid (track count + auto-thumbnail may have
                        // changed for a previously-empty playlist).
                        state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));
                    }
                    Err(e) => log::warn!("import_files_to_playlist (drop): {e}"),
                }
            }
        }
    });
}
