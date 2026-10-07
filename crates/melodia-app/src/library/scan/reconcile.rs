//! Passes that scan library folders back to back, and how a request for one merges into the pass
//! already running.

use std::cmp::Reverse;
use std::path::Path;

use parking_lot::Mutex;
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
        let in_reach = match self {
            Self::Library => true,
            Self::UnfinishedImports => folder.last_scanned.is_none(),
        };
        folder.is_enabled && in_reach
    }
}

/// Coalesces concurrent reconcile triggers. A rapid-fire kernel-overflow burst during `rsync`
/// could otherwise spawn three full library sweeps back to back; each one is idempotent but would
/// still walk and stat every file. One lock, so a request can't land between a pass deciding it is
/// done and letting go, which is where a deferred one would be lost.
static PASS: Mutex<PassState> = Mutex::new(PassState::Idle);

enum PassState {
    Idle,
    Running {
        /// A library-wide pass asked for while this one ran. A narrower round doesn't read the
        /// folders that request is for, so it runs one next rather than dropping it.
        library_owed: bool,
    },
}

/// This task's hold on [`PASS`], let go on any way out, a panic included.
struct PassClaim {
    held: bool,
}

impl PassClaim {
    /// Claims [`PASS`], or merges `reach` into the pass holding it.
    fn take(reach: Reach) -> Option<Self> {
        let mut pass = PASS.lock();
        if let PassState::Running { library_owed } = &mut *pass {
            *library_owed |= reach == Reach::Library;
            return None;
        }
        *pass = PassState::Running { library_owed: false };
        Some(Self { held: true })
    }

    /// The round owed after one over `finished`, letting go under the same lock when none is.
    fn next_round(&mut self, finished: Reach) -> Option<Reach> {
        let mut pass = PASS.lock();
        let owed = matches!(*pass, PassState::Running { library_owed: true });
        if owed && finished != Reach::Library {
            *pass = PassState::Running { library_owed: false };
            return Some(Reach::Library);
        }
        *pass = PassState::Idle;
        self.held = false;
        None
    }
}

impl Drop for PassClaim {
    fn drop(&mut self) {
        if self.held {
            *PASS.lock() = PassState::Idle;
        }
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
/// again over the whole library once it ends.
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
    let Some(mut claim) = PassClaim::take(reach) else {
        log::debug!("reconcile_watched_folders: already in flight, merging into it");
        return;
    };
    let spawner = TaskSpawner::from_state(state);
    let state = state.clone();
    let cancel = state.scan.token();
    spawner.spawn(async move {
        let _handed_over = restoring;
        let mut reach = reach;
        loop {
            reach = round(&state, reach, &cancel).await;
            if cancel.is_cancelled() {
                return;
            }
            let Some(next) = claim.next_round(reach) else {
                return;
            };
            reach = next;
        }
    });
}

/// One round over the folders `reach` covers, answering the reach it ended up reading: the repair
/// widens it to the library when it clears covers.
async fn round(state: &AppState, reach: Reach, cancel: &CancellationToken) -> Reach {
    let mut folders = match queries::folder::get_all_folders(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!("reconcile_watched_folders: load folders failed: {}", describe(&e));
            return reach;
        }
    };
    if reach == Reach::UnfinishedImports && !folders.iter().any(|f| reach.covers(f)) {
        // The repair runs ahead of a scan, and with watching off there is none to run ahead of.
        return reach;
    }
    let restoring = forget_missing_artwork(state).await;
    // The repair cleared covers across the library, and a cleared one is never reported missing
    // again, so only a full round brings them back.
    let reach = if restoring.is_some() { Reach::Library } else { reach };
    folders.retain(|f| reach.covers(f));
    scan_in_turn(state, folders, cancel).await;
    reach
}

/// Scans `folders` one after another, until `cancel` stops the pass.
async fn scan_in_turn(state: &AppState, mut folders: Vec<Folder>, cancel: &CancellationToken) {
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
