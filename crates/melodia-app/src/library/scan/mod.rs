//! The library scan: one folder's walk, read and write, and the reconcile that runs it over several
//! in turn.
//!
//! Every scan stops on [`ScanControl`](crate::state::ScanControl)'s token, for [`cancel`] and quit
//! alike, at the walk, the filter, each file read and each chunk's head. **A stopped scan keeps
//! what it read and deletes nothing**, except the import of a folder just added, which a cancel
//! takes back out ([`OnStop::Withdraw`]). It skips the orphan purge and the scanned stamp, and the
//! next scan reads only the rest, the size and mtime gate skipping what is stored.
//!
//! A scan leaves alone the files of a library folder nested inside its own until a completed scan
//! absorbs it.

mod finish;
mod reconcile;
mod repair;
mod run;

pub use reconcile::{finish_interrupted_imports, reconcile_watched_folders};
pub use repair::restore_missing_artwork;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::library::settings::folders::{NestedFolder, folders_inside};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::entities::folder::Folder;
use melodia_core::entities::scan::ScannedFile;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::toast::{self, ToastKind};
use melodia_store::database::queries::ingest::Ingested;
use melodia_store::database::{DbPool, queries};
use melodia_store::media::ingest::scan_pool::ScanPool;
use melodia_store::media::ingest::scanner::{
    ScanObserver, WalkIndex, collect_media_files, scan_files_parallel,
};
use reconcile::Reach;
use repair::RestoreNotice;
use run::ScanRun;

/// How a scan ended, when it didn't fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanOutcome {
    Completed {
        inserted: u32,
    },
    /// Cancelled, or interrupted by shutdown, after keeping whatever it had read.
    Stopped {
        /// Whether it rewrote tracks the library held before it started, which a withdraw would
        /// delete along with the folder.
        rewrote_existing: bool,
    },
    /// Cancelled under [`OnStop::Withdraw`], which took the folder back out of the library.
    Withdrawn,
}

/// What a scan the user cancels leaves behind. Quitting always keeps, and an import it cuts short
/// is finished at the next launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnStop {
    /// What was read stays: the folder was in the library before this scan, so a cancel only stops
    /// the refresh.
    Keep,
    /// The folder goes, with everything the scan brought in: for a folder just added, Cancel means
    /// not adding it, and half of it would leave the next reconcile to finish what was cancelled.
    /// An import that has taken over tracks from elsewhere in the library stays, since removing it
    /// would delete them.
    Withdraw,
}

/// Scans one folder, stopping when [`cancel`] or shutdown asks.
///
/// Repairs first, and when the repair cleared covers it hands the rest of the library to a
/// reconcile, since this scan only re-reads its own folder.
pub async fn scan_folder(
    state: &AppState,
    folder_id: i64,
    on_stop: OnStop,
) -> Result<ScanOutcome, AppError> {
    // Read ahead of the token, so a cancel landing between the two can't stop this scan unseen.
    let cancels_at_start = state.scan.user_cancels();
    let cancel = state.scan.token();
    let restoring = forget_missing_artwork(state).await;
    let run = ScanRun::start(&state.scan, cancel);
    let outcome = scan_one(state, folder_id, &run).await?;
    let cancelled_import =
        on_stop == OnStop::Withdraw && state.scan.user_cancels() != cancels_at_start;
    let outcome = match outcome {
        ScanOutcome::Stopped { rewrote_existing: false } if cancelled_import => {
            // Under the run, so the bar stays on "Stopping" until the library is back as it was.
            crate::library::settings::remove_folder(state, folder_id).await?;
            log::info!("Withdrew folder {folder_id}, its import having been cancelled");
            ScanOutcome::Withdrawn
        }
        outcome @ ScanOutcome::Stopped { rewrote_existing: true } if cancelled_import => {
            log::info!(
                "Kept folder {folder_id}: its cancelled import had taken over existing tracks"
            );
            outcome
        }
        outcome => outcome,
    };
    // Lets go of the bar before a reconcile puts up its own.
    drop(run);
    // The repair cleared missing covers across the whole library and this folder has re-read
    // only its own share, so the rest follow under the same notice.
    if let Some(notice) = restoring
        && matches!(outcome, ScanOutcome::Completed { .. })
    {
        reconcile::start(state, Reach::Library, Some(notice));
    }
    Ok(outcome)
}

/// The repair every scan entry runs before reading anything. A failure is logged rather than
/// returned: a scan that couldn't check the artwork store is still worth running.
async fn forget_missing_artwork(state: &AppState) -> Option<RestoreNotice> {
    let repaired = repair::forget_missing(&state.db, &state.paths, &state.artwork_restoring).await;
    repaired.unwrap_or_else(|e| {
        log::warn!("Artwork check before the scan failed: {}", describe(&e));
        None
    })
}

/// Scans one folder in the background, tracked so shutdown waits for its last write. A failure
/// is logged and toasted, there being nobody left to hand it to.
pub fn start(state: &AppState, folder_id: i64, on_stop: OnStop) {
    let spawner = TaskSpawner::from_state(state);
    let state = state.clone();
    spawner.spawn(async move {
        if let Err(e) = scan_folder(&state, folder_id, on_stop).await {
            log::warn!("Scan of folder {folder_id} failed: {}", describe(&e));
            toast::notify(ToastKind::OperationFailed, describe(&e));
        }
    });
}

/// Stops every scan running now. What each one has read stays in the library, unless its
/// [`OnStop`] withdraws the folder.
pub fn cancel(state: &AppState) {
    state.scan.cancel();
}

/// Files parsed and ingested per write transaction: large enough that begin, commit and the trigger
/// DDL are noise, small enough that interactive writes get the single writer every few seconds on
/// a slow disk. Also the scan's memory peak, one chunk's tags being resident at a time.
const TX_CHUNK_FILES: usize = 2_000;

async fn scan_one(
    state: &AppState,
    folder_id: i64,
    run: &Arc<ScanRun>,
) -> Result<ScanOutcome, AppError> {
    let folder = queries::folder::get_folder_by_id(&state.db, folder_id).await?;

    if !Path::new(&folder.path).exists() {
        return Err(AppError::scanner_msg(format!("Folder does not exist: {}", folder.path)));
    }
    let scope = Arc::new(ScanScope::of(&state.db, &folder).await?);

    let Some(Discovery { walked, to_scan, orphans, pool }) =
        discover(&state.db, folder.id, Arc::clone(&scope), Arc::clone(run)).await?
    else {
        return Ok(ScanOutcome::Stopped { rewrote_existing: false });
    };
    if walked == 0 {
        finish::after_completed(state, folder.id).await?;
        return Ok(ScanOutcome::Completed { inserted: 0 });
    }

    let skipped = walked - to_scan.len();
    if skipped > 0 {
        log::info!(
            "Incremental scan of '{}': {skipped} file(s) skipped as unchanged or a nested folder's, \
             {} to (re)parse",
            folder.path,
            to_scan.len()
        );
    }

    // Decided before the chunk loop consumes `to_scan`. The orphans count too, so a purge of
    // most of a folder runs with the triggers dropped rather than paying them per row.
    let mut ingested = Ingested::for_delta(to_scan.len() + orphans.len());
    run.begin_reading(u32::try_from(to_scan.len()).unwrap_or(u32::MAX));

    let scan_timestamp = melodia_core::utils::now_rfc3339();

    // --- Stage 1: a write transaction per chunk, so interactive writes don't queue behind a long
    // first scan. `library_changed` bumps only after the final commit, and a crash between chunks
    // leaves committed tracks the next scan's gate skips. A cancel mid-read still writes what it
    // read.
    let mut remaining = to_scan.into_iter();
    while !run.is_cancelled() {
        let chunk: Vec<PathBuf> = remaining.by_ref().take(TX_CHUNK_FILES).collect();
        if chunk.is_empty() {
            break;
        }
        let chunk_len = u32::try_from(chunk.len()).unwrap_or(u32::MAX);
        let scanned_files = parse_chunk(state, chunk, &pool, Arc::clone(run)).await?;
        run.chunk_read(chunk_len);
        if scanned_files.is_empty() {
            continue;
        }

        let result = queries::ingest::commit_scan_chunk(
            &state.db,
            &scanned_files,
            folder.id,
            &scan_timestamp,
            &pool,
            &ingested,
        )
        .await?;
        ingested.add(&result);
    }
    drop(pool);

    // --- Stage 2. `queries::scan::commit_scan` argues what a stopped scan still owes.
    if !run.try_begin_finishing() {
        if ingested.any() {
            queries::scan::commit_scan(&state.db, &folder, &[], &ingested).await?;
            state.library_changed.bump();
        }
        return Ok(ScanOutcome::Stopped { rewrote_existing: ingested.rewrote_existing() });
    }
    queries::scan::commit_scan(&state.db, &folder, &orphans, &ingested).await?;
    finish::absorb_nested(&state.db, &folder, &scope.nested).await?;
    finish::after_completed(state, folder.id).await?;
    // Only for a change: every list showing the library re-fetches on a bump, and a launch scans
    // every folder. The folder's new `last_scanned` reaches Settings when the run ends instead.
    if ingested.any() || !orphans.is_empty() || !scope.nested.is_empty() {
        state.library_changed.bump();
    }

    Ok(ScanOutcome::Completed { inserted: ingested.inserted })
}

/// What a folder's scan reads: its directory, less the library folders nested inside it.
struct ScanScope {
    root: PathBuf,
    nested: Vec<NestedFolder>,
}

impl ScanScope {
    /// On the blocking pool, since telling which folders are nested costs a stat apiece.
    async fn of(db: &DbPool, folder: &Folder) -> Result<Self, AppError> {
        let folders = queries::folder::get_all_folders(db).await?;
        let root = PathBuf::from(&folder.path);
        tokio::task::spawn_blocking(move || {
            let nested = folders_inside(&root, &folders);
            Self { root, nested }
        })
        .await
        .map_err(|e| AppError::scanner("Scan scope task failed", e))
    }

    fn owns(&self, path: &Path) -> bool {
        !self.nested.iter().any(|folder| folder.is_enabled && path.starts_with(&folder.path))
    }
}

/// What the walk found, and what the incremental filter left to read.
struct Discovery {
    /// How many audio files the walk found.
    walked: usize,
    to_scan: Vec<PathBuf>,
    /// The folder's rows the walk didn't find, for the commit to purge.
    orphans: Vec<String>,
    /// One pool serves the sort, the filter, the parse and the ingest's stat, and ends with the
    /// ingest.
    pool: ScanPool,
}

/// Walks the folder, pairs each file with its row, and filters the walk to the files that are new
/// or whose size or mtime moved, or answers `None` once the scan is cancelled.
///
/// The walk and the filter are synchronous syscall loops, so they go to the blocking pool rather
/// than pin a runtime worker through a cold-cache boot reconcile. The rows are read between them
/// one at a time against the sorted walk, so the folder's paths are never held twice. The filter
/// runs on Rayon: every file reaches its `stat` and almost none pass, which makes it the whole cost
/// of the common nothing-changed rescan. `to_scan` comes out in path order.
async fn discover(
    db: &DbPool,
    folder_id: i64,
    scope: Arc<ScanScope>,
    run: Arc<ScanRun>,
) -> Result<Option<Discovery>, AppError> {
    let walk_scope = Arc::clone(&scope);
    let walk_run = Arc::clone(&run);
    let walked = tokio::task::spawn_blocking(move || {
        let walk = collect_media_files(&walk_scope.root, walk_run.as_ref())?;
        let pool = ScanPool::for_files(walk.files.len());
        let index = pool.install(|| WalkIndex::new(walk));
        Some((index, pool))
    })
    .await
    .map_err(|e| AppError::scanner("Scan walk task failed", e))?;
    let Some((mut index, pool)) = walked else {
        return Ok(None);
    };

    // `scan_one` purges nothing after an empty walk, so its rows would be read only to be dropped.
    if !index.is_empty() {
        queries::scan::visit_folder_tracks(db, folder_id, |path, row| index.add_row(path, row))
            .await?;
    }

    tokio::task::spawn_blocking(move || {
        // Reads only what this folder owns, but the walk stays whole: the purge checks the
        // folder's rows against it, and some can sit inside a nested folder's directory.
        let to_scan =
            pool.install(|| index.changed(|path| !run.is_cancelled() && scope.owns(path)));
        if run.is_cancelled() {
            return None;
        }
        Some(Discovery { walked: index.len(), to_scan, orphans: index.into_orphans(), pool })
    })
    .await
    .map_err(|e| AppError::scanner("Scan filter task failed", e))
}

/// Parses one chunk of a scan on its pool, reporting through `run`.
async fn parse_chunk(
    state: &AppState,
    paths: Vec<PathBuf>,
    pool: &ScanPool,
    run: Arc<ScanRun>,
) -> Result<Vec<ScannedFile>, AppError> {
    let artwork_dir = state.paths.artwork_dir.clone();
    let cover_cache = state.cover_cache.clone();
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        pool.install(|| scan_files_parallel(&paths, &artwork_dir, &cover_cache, run.as_ref()))
    })
    .await
    .map_err(|e| AppError::scanner("Scan task failed", e))
}

#[cfg(test)]
#[path = "tests/scan_tests.rs"]
mod tests;
