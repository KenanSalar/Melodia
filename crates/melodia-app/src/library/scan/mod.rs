//! The library scan: one folder's walk, read and write, and the reconcile that runs it over several
//! in turn.
//!
//! Every scan stops on a token from [`ScanControl`](crate::state::ScanControl), so [`cancel`]
//! stops any of them, and quitting stops them at the same checkpoints: the walk, the incremental
//! filter, each file of a read, and the head of every chunk. **A stopped scan keeps what it read
//! and deletes nothing**, the one exception being the import of a folder just added, which a
//! cancel takes back out ([`OnStop::Withdraw`]). Otherwise committed chunks stay, and so does the
//! part of the chunk read when the cancel landed. The orphan purge doesn't run, its walk being
//! incomplete, and the folder isn't stamped as scanned. The next scan of the folder reads only the
//! rest, the size and mtime gate skipping everything already stored.
//!
//! A scan leaves alone the files of a library folder nested inside its own, which stay that
//! folder's until a completed scan absorbs it.

mod finish;
mod reconcile;
mod repair;
mod run;

pub use reconcile::{finish_interrupted_imports, reconcile_watched_folders};
pub use repair::restore_missing_artwork;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;

use crate::library::settings::folders::{NestedFolder, folders_inside};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use finish::Ingested;
use melodia_core::entities::folder::Folder;
use melodia_core::entities::scan::{ExistingTrackSummary, ScannedFile};
use melodia_core::error::{AppError, describe};
use melodia_core::utils::toast::{self, ToastKind};
use melodia_store::database::queries;
use melodia_store::media::ingest::scan_pool::ScanPool;
use melodia_store::media::ingest::scanner::{
    MediaWalk, ScanObserver, collect_media_files, scan_files_parallel, track_is_current,
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
    /// The folder goes, with everything the scan brought in. For the import of a folder just
    /// added, where Cancel means not adding it, and keeping half of it would also leave the next
    /// reconcile to finish what the user cancelled. An import that has already taken over tracks
    /// from elsewhere in the library stays, since removing it would delete them.
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
            toast::notify(ToastKind::OperationFailed, e.to_string());
        }
    });
}

/// Stops every scan running now. What each one has read stays in the library, unless its
/// [`OnStop`] withdraws the folder.
pub fn cancel(state: &AppState) {
    state.scan.cancel();
}

/// Scan-delta size above which the denormalized-stats triggers are dropped
/// for the ingest and rebuilt via one `recalculate_all_stats` sweep at the
/// end. At or below it the triggers stay enabled: per-row maintenance on a
/// handful of inserts/updates/deletes is far cheaper than the full 3-table
/// correlated-subquery recalc the drop would force. Same value and
/// rationale as the watcher reconcile path's `BULK_THRESHOLD`
/// (`tasks/file_event_processor/reconcile.rs`).
const SCAN_BULK_THRESHOLD: usize = 20;

/// Files a scan parses and then ingests in one write transaction. Large
/// enough that per-chunk overhead (begin/commit + the 6 trigger DDL
/// statements) is noise, small enough that interactive writes waiting on the
/// single writer connection get a slot every few seconds even on slow disks.
/// It is also the scan's memory peak: one chunk's parsed tags are resident at
/// a time, however large the library.
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
    let scope = Arc::new(ScanScope::of(state, &folder).await?);

    // Read-side pre-load through the read pool (before the writer tx opens):
    // size + mtime for every track already in this folder. Doesn't contend
    // with the scan's writes.
    let existing_summaries =
        queries::scan::get_existing_track_summaries_for_folder(&state.db, folder.id).await?;

    let Some(Discovery { walk, to_scan, pool }) =
        discover(Arc::clone(&scope), existing_summaries, Arc::clone(run)).await?
    else {
        return Ok(ScanOutcome::Stopped { rewrote_existing: false });
    };
    if walk.files.is_empty() {
        finish::after_completed(state, folder.id).await?;
        return Ok(ScanOutcome::Completed { inserted: 0 });
    }

    let skipped = walk.files.len() - to_scan.len();
    if skipped > 0 {
        log::info!(
            "Incremental scan of '{}': {skipped} file(s) skipped as unchanged or a nested folder's, \
             {} to (re)parse",
            folder.path,
            to_scan.len()
        );
    }

    // Decided before the chunk loop consumes `to_scan`. Edge: a tiny
    // `to_scan` combined with a huge orphan purge (folder emptied
    // externally) runs the delete trigger per orphaned row — rare, still
    // correct, and accepted over plumbing the orphan count (unknown until
    // inside the transaction) into this decision.
    let mut ingested = Ingested::new(to_scan.len() > SCAN_BULK_THRESHOLD);
    run.begin_reading(u32::try_from(to_scan.len()).unwrap_or(u32::MAX));

    let scan_timestamp = melodia_core::utils::now_rfc3339();

    // --- Stage 1: parse and ingest a chunk at a time, each chunk in a write
    // transaction of its own. The single writer connection frees between
    // chunks, so interactive writes (favorite toggles, play-count flushes,
    // position saves) don't queue behind a multi-minute first scan. Each
    // chunk is self-consistent: stats triggers are dropped and recreated
    // INSIDE its transaction, so a crash never leaves them missing — the
    // stats merely lag until the final recalc below, which is invisible to
    // the UI because `library_changed` is bumped only after the final
    // commit. A crash between chunks leaves committed tracks behind; the
    // next scan's size+mtime gate makes the re-run a cheap no-op over them.
    //
    // A cancel mid-read leaves the rest of that chunk unread, and what was
    // read is still written: a stop costs at most this one transaction.
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

        let mut tx = state.db.write().begin().await?;
        if ingested.is_bulk() {
            queries::stats::disable_stats_triggers(&mut tx).await?;
        }
        let result = queries::ingest::ingest_scanned_files(
            &mut tx,
            &scanned_files,
            &queries::FolderResolution::Fixed(folder.id),
            &scan_timestamp,
            true,
            &pool,
        )
        .await?;
        if ingested.is_bulk() {
            queries::stats::enable_stats_triggers(&mut tx).await?;
        }
        tx.commit().await?;
        ingested.add(&result);
    }
    drop(pool);

    // --- Stage 2. `finish::commit_final` argues what a stopped scan still owes.
    if !run.try_begin_finishing() {
        if ingested.any() {
            finish::commit_final(state, &folder, None, &ingested).await?;
            state.library_changed.bump();
        }
        return Ok(ScanOutcome::Stopped { rewrote_existing: ingested.rewrote_existing() });
    }
    finish::commit_final(state, &folder, Some(walk), &ingested).await?;
    finish::absorb_nested(state, &folder, &scope.nested).await?;
    finish::after_completed(state, folder.id).await?;
    state.library_changed.bump();

    Ok(ScanOutcome::Completed { inserted: ingested.inserted })
}

/// What a folder's scan reads: its directory, less the library folders nested inside it.
struct ScanScope {
    root: PathBuf,
    nested: Vec<NestedFolder>,
}

impl ScanScope {
    /// On the blocking pool, since telling which folders are nested costs a stat apiece.
    async fn of(state: &AppState, folder: &Folder) -> Result<Self, AppError> {
        let folders = queries::folder::get_all_folders(&state.db).await?;
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
    walk: MediaWalk,
    to_scan: Vec<PathBuf>,
    /// One pool serves the filter, the parse and the ingest's stat, and ends with the ingest.
    pool: ScanPool,
}

/// Walks the folder and filters it down to the files that need reading, or answers `None` once
/// the scan is cancelled.
///
/// Both halves go to the blocking pool. They are synchronous syscall loops (`WalkDir` over the
/// whole tree, then one `fs::metadata` per already-known file inside `track_is_current`), and
/// inline in the async caller they would pin one of the runtime's few workers for the duration
/// on a cold-cache disk, stalling position ticks and watcher deliveries during boot reconciles.
///
/// The incremental filter only (re)parses files that are new, or whose
/// size or mtime no longer matches the stored row. Byte-unchanged files
/// keep their existing DB metadata untouched — Lofty is skipped for
/// them entirely, which is the bulk of a typical startup rescan.
///
/// The filter runs on Rayon (like `scan_files_parallel` does downstream):
/// *every* file in the library reaches it and almost none proceed past it, so
/// its per-file `stat` is what a rescan-with-nothing-changed — the common case
/// — actually spends its time on, and a serial syscall loop is the worst shape
/// for it on a cold cache or a network mount. Rayon's `collect` preserves the
/// sequential order, so `to_scan` stays byte-for-byte what it was before.
async fn discover(
    scope: Arc<ScanScope>,
    existing: HashMap<String, ExistingTrackSummary>,
    run: Arc<ScanRun>,
) -> Result<Option<Discovery>, AppError> {
    tokio::task::spawn_blocking(move || {
        let walk = collect_media_files(&scope.root, run.as_ref())?;
        let pool = ScanPool::for_files(walk.files.len());
        let to_scan: Vec<PathBuf> = pool.install(|| {
            // Reads only what this folder owns, but the walk stays whole: the purge checks the
            // folder's rows against it, and some can sit inside a nested folder's directory.
            walk.files
                .par_iter()
                .filter(|path| {
                    !run.is_cancelled() && scope.owns(path) && !track_is_current(path, &existing)
                })
                .cloned()
                .collect()
        });
        if run.is_cancelled() {
            return None;
        }
        Some(Discovery { walk, to_scan, pool })
    })
    .await
    .map_err(|e| AppError::scanner("Scan walk task failed", e))
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
