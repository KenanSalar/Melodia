//! Passes that scan library folders back to back, and how a request for one merges into the pass
//! already running.

use std::cmp::Reverse;
use std::path::Path;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::finish::purge_held;
use super::repair::RestoreNotice;
use super::run::ScanRun;
use super::{Orphans, forget_missing_artwork, scan_one};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::entities::folder::Folder;
use melodia_core::error::describe;
use melodia_platform::services::platform::allocator::trim;
use melodia_store::database::queries;
use melodia_store::database::queries::scan::Unwalked;

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

/// Coalesces concurrent reconcile triggers, so a watcher-overflow burst during `rsync` merges into
/// one round after the running one. One lock, so a request can't land between a pass deciding it
/// is done and letting go, where a deferred one would be lost.
static PASS: Mutex<PassState> = Mutex::new(PassState::Idle);

enum PassState {
    Idle,
    Running(Owed),
}

/// What the requests that merged into a running pass are still waiting for.
#[derive(Default, Clone, Copy)]
struct Owed {
    /// A library-wide pass asked for while this one ran. A narrower round doesn't read the folders
    /// that request is for, so it runs one next rather than dropping it.
    library: bool,
    /// The watcher missed events while this one ran, for a moved directory or an overflow. A walk
    /// already past the change never sees it, so even a full round is followed by one that starts
    /// after it.
    rewalk: bool,
}

/// This task's hold on a pass lock, let go on any way out, a panic included.
struct PassClaim<'a> {
    pass: &'a Mutex<PassState>,
    held: bool,
}

impl<'a> PassClaim<'a> {
    /// Claims `pass`, or merges `reach` into the pass holding it.
    fn take(pass: &'a Mutex<PassState>, reach: Reach) -> Option<Self> {
        Self::claim_or_merge(pass, |owed| owed.library |= reach == Reach::Library)
    }

    /// Claims `pass`, or leaves the pass holding it owing a round that starts after this call.
    fn take_for_rewalk(pass: &'a Mutex<PassState>) -> Option<Self> {
        Self::claim_or_merge(pass, |owed| owed.rewalk = true)
    }

    fn claim_or_merge(pass: &'a Mutex<PassState>, merge: impl FnOnce(&mut Owed)) -> Option<Self> {
        let mut state = pass.lock();
        if let PassState::Running(owed) = &mut *state {
            merge(owed);
            return None;
        }
        *state = PassState::Running(Owed::default());
        Some(Self { pass, held: true })
    }

    /// The round owed after one over `finished`, letting go under the same lock when none is.
    fn next_round(&mut self, finished: Reach) -> Option<Reach> {
        let mut state = self.pass.lock();
        let owed = match *state {
            PassState::Running(owed) => owed,
            PassState::Idle => Owed::default(),
        };
        if owed.rewalk || (owed.library && finished != Reach::Library) {
            *state = PassState::Running(Owed::default());
            return Some(Reach::Library);
        }
        *state = PassState::Idle;
        self.held = false;
        None
    }
}

impl Drop for PassClaim<'_> {
    fn drop(&mut self) {
        if self.held {
            *self.pass.lock() = PassState::Idle;
        }
    }
}

/// Background reconcile of every enabled library folder: the watcher reports only live events, so
/// what changed while it was off arrives through a pass like this or not at all.
///
/// Sequential per folder, `SQLite` having one writer and the scan progress one `watch` slot. One
/// token for the whole pass, so a cancel stops the folders still to come too.
///
/// A call while a pass runs merges into it. A full pass answers with what it has yet to walk, and
/// its artwork repair ran at its start, so a cover gone since waits for the next scan entry; a
/// narrower pass runs again over the whole library once it ends.
pub fn reconcile_watched_folders(state: &AppState) {
    start(state, Reach::Library, None);
}

/// [`reconcile_watched_folders`] for changes the watcher couldn't report file by file, a moved
/// directory or an overflow: nothing else will tell of them, so a running pass owes a round that
/// starts after this call, whatever its reach.
pub fn reconcile_after_missed_events(state: &AppState) {
    match PassClaim::take_for_rewalk(&PASS) {
        Some(claim) => run_pass(state, claim, Reach::Library, None),
        None => log::debug!("reconcile_after_missed_events: already in flight, owed a round"),
    }
}

/// Finishes the imports a quit cut short: the launch's pass when watching is off.
pub fn finish_interrupted_imports(state: &AppState) {
    start(state, Reach::UnfinishedImports, None);
}

/// Starts a pass over the folders `reach` covers, holding up a restore notice a scan has already
/// raised until it ends.
pub(super) fn start(state: &AppState, reach: Reach, restoring: Option<RestoreNotice>) {
    let Some(claim) = PassClaim::take(&PASS, reach) else {
        log::debug!("reconcile_watched_folders: already in flight, merging into it");
        return;
    };
    run_pass(state, claim, reach, restoring);
}

fn run_pass(
    state: &AppState,
    mut claim: PassClaim<'static>,
    reach: Reach,
    restoring: Option<RestoreNotice>,
) {
    let spawner = TaskSpawner::from_state(state);
    let state = state.clone();
    let cancel = state.scan.token();
    spawner.spawn(async move {
        let _handed_over = restoring;
        let mut reach = reach;
        loop {
            let ended = round(&state, reach, &cancel).await;
            if cancel.is_cancelled() {
                return;
            }
            // An owed round's walk finds its own orphans, and one of these may be a file moved to
            // a path this round's walk had already passed.
            let Some(next) = claim.next_round(ended.reach) else {
                if let Err(e) = purge_held(&state, ended.orphans).await {
                    log::warn!("reconcile_watched_folders: orphan purge failed: {}", describe(&e));
                }
                break;
            };
            reach = next;
        }
        // Each walk holds a path and a row per file and frees them all at once. On a large library
        // that outlasts the startup trim, so the pass hands its own slack back.
        trim();
    });
}

struct RoundEnd {
    /// The reach it ended up reading: the repair widens it to the library when it clears covers.
    reach: Reach,
    orphans: Unwalked,
}

impl RoundEnd {
    fn without_scanning(reach: Reach) -> Self {
        Self { reach, orphans: Unwalked::default() }
    }
}

/// One round over the folders `reach` covers.
async fn round(state: &AppState, reach: Reach, cancel: &CancellationToken) -> RoundEnd {
    let mut folders = match queries::folder::get_all_folders(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!("reconcile_watched_folders: load folders failed: {}", describe(&e));
            return RoundEnd::without_scanning(reach);
        }
    };
    if reach == Reach::UnfinishedImports && !folders.iter().any(|f| reach.covers(f)) {
        // The repair runs ahead of a scan, and with watching off there is none to run ahead of.
        return RoundEnd::without_scanning(reach);
    }
    let restoring = forget_missing_artwork(state).await;
    // The repair cleared covers across the library, and a cleared one is never reported missing
    // again, so only a full round brings them back.
    let reach = if restoring.is_some() { Reach::Library } else { reach };
    folders.retain(|f| reach.covers(f));
    let orphans = scan_in_turn(state, folders, cancel).await;
    RoundEnd { reach, orphans }
}

/// Scans `folders` one after another, until `cancel` stops the pass, answering the orphans they
/// held back.
async fn scan_in_turn(
    state: &AppState,
    mut folders: Vec<Folder>,
    cancel: &CancellationToken,
) -> Unwalked {
    // Deepest first: a folder absorbs the ones nested in it when its scan completes, so
    // theirs has to run before it or not at all.
    folders.sort_by_key(|f| Reverse(Path::new(&f.path).components().count()));
    let mut orphans = Unwalked::default();
    for folder in &folders {
        if cancel.is_cancelled() {
            log::info!("reconcile_watched_folders: stopped, bailing between folders");
            break;
        }
        let run = ScanRun::start(&state.scan, cancel.clone());
        if let Err(e) = scan_one(state, folder.id, &run, Orphans::HoldIn(&mut orphans)).await {
            log::warn!(
                "reconcile_watched_folders: scan of {} failed: {}",
                folder.path,
                describe(&e)
            );
        }
    }
    orphans
}

#[cfg(test)]
#[path = "tests/reconcile_tests.rs"]
mod tests;
