//! Passes that scan library folders back to back, and how a request for one merges into the pass
//! already running.

use std::cmp::Reverse;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio_util::sync::CancellationToken;

use super::repair::RestoreNotice;
use super::run::ScanRun;
use super::{forget_missing_artwork, scan_one};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::entities::folder::Folder;
use melodia_core::error::describe;
use melodia_store::database::queries;

/// Which folders a pass reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// The folders no scan has completed, `last_scanned` being stamped by a completed scan alone.
    UnfinishedImports,
    Library,
}

impl Reach {
    fn covers(self, folder: &Folder) -> bool {
        folder.is_enabled && (self == Self::Library || folder.last_scanned.is_none())
    }
}

/// Coalesces concurrent reconcile triggers. A rapid-fire kernel-overflow
/// burst during `rsync` could otherwise spawn three full library sweeps
/// back-to-back; each one is idempotent but would still walk and stat every
/// file. The flag is RAII-cleared by [`ReconcileGuard`] so a panic or
/// early-return path can't strand it.
static RECONCILE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// A library-wide pass asked for while a narrower one ran, which doesn't read the folders that
/// request is for, so it is deferred rather than dropped. Cleared by whoever claims
/// [`RECONCILE_IN_FLIGHT`], so a debt a full pass has already paid isn't paid twice.
static LIBRARY_PASS_OWED: AtomicBool = AtomicBool::new(false);

struct ReconcileGuard;
impl Drop for ReconcileGuard {
    fn drop(&mut self) {
        RECONCILE_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// Fire-and-forget background reconcile of every enabled library folder.
/// Catches files that landed (or vanished) while the watcher was off: the
/// watcher itself only reports live events, so what changed while it was
/// off reaches the library through a pass like this or not at all.
/// Triggered after the watcher transitions off → on
/// (`resume_watching::run`, `toggle_folder_watching(true)`), on
/// watcher-overflow `RescanNeeded` from `file_event_processor`, by
/// `tag_backfill`, by `artwork_restore`, and by a completed
/// [`scan_folder`](super::scan_folder) whose repair cleared any covers.
///
/// Sequential per folder: `SQLite` has a single writer, and the scan
/// progress is one `watch` slot that parallel scans would clobber. Each
/// folder's scan already bumps `library_changed`, so Browse/Tracks views
/// auto-refresh per folder as scans complete.
///
/// One token for the whole pass, so a cancel stops the folders still to
/// come as well as the one being scanned. Tracked on `TaskSpawner` so
/// shutdown waits for the in-flight folder's last write.
///
/// A call while a pass is in flight merges into it. A full pass still reads
/// the latest audio files, but its artwork repair ran at its start, so a
/// cover gone since waits for the next scan entry. A narrower pass runs
/// again over the whole library once it ends ([`LIBRARY_PASS_OWED`]).
pub fn reconcile_watched_folders(state: &AppState) {
    start(state, Reach::Library, None);
}

/// Finishes the imports a quit cut short: the launch's pass when watching is off.
pub fn finish_interrupted_imports(state: &AppState) {
    start(state, Reach::UnfinishedImports, None);
}

/// Starts a pass over the folders `reach` covers, holding up a restore notice a scan has already
/// raised until it ends.
pub(super) fn start(state: &AppState, reach: Reach, restoring: Option<RestoreNotice>) {
    if RECONCILE_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        if reach == Reach::Library {
            LIBRARY_PASS_OWED.store(true, Ordering::Release);
        }
        log::debug!("reconcile_watched_folders: already in flight, merging into it");
        return;
    }
    LIBRARY_PASS_OWED.store(false, Ordering::Release);
    let spawner = TaskSpawner::from_state(state);
    let state = state.clone();
    let cancel = state.scan.token();
    spawner.spawn(async move {
        let _guard = ReconcileGuard;
        let _handed_over = restoring;
        let mut reach = reach;
        loop {
            let restoring = forget_missing_artwork(&state).await;
            if restoring.is_some() {
                // The repair cleared covers across the library, and a cleared one is never
                // reported missing again, so only a full round brings them back.
                reach = Reach::Library;
            }
            scan_in_turn(&state, reach, &cancel).await;
            if cancel.is_cancelled()
                || reach == Reach::Library
                || !LIBRARY_PASS_OWED.swap(false, Ordering::AcqRel)
            {
                return;
            }
            reach = Reach::Library;
        }
    });
}

/// Scans the folders `reach` covers one after another, until `cancel` stops the pass.
async fn scan_in_turn(state: &AppState, reach: Reach, cancel: &CancellationToken) {
    let mut folders = match queries::folder::get_all_folders(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!("reconcile_watched_folders: load folders failed: {}", describe(&e));
            return;
        }
    };
    folders.retain(|f| reach.covers(f));
    // Deepest first: a folder absorbs the ones nested in it when its scan completes, so
    // theirs has to run before it or not at all.
    folders.sort_by_key(|f| Reverse(Path::new(&f.path).components().count()));
    for folder in &folders {
        if cancel.is_cancelled() {
            log::info!("reconcile_watched_folders: stopped, bailing between folders");
            return;
        }
        let run = ScanRun::start(state, cancel.clone());
        if let Err(e) = scan_one(state, folder.id, &run).await {
            log::warn!(
                "reconcile_watched_folders: scan of {} failed: {}",
                folder.path,
                describe(&e)
            );
        }
    }
}
