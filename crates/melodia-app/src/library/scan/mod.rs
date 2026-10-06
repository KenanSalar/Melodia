//! The library scan: one folder's walk, read and write, and the reconcile that runs it over every
//! enabled folder.
//!
//! Every scan stops on a token from [`ScanControl`](crate::state::ScanControl), so [`cancel`]
//! stops any of them, and quitting stops them at the same checkpoints: the walk, the incremental
//! filter, each file of a read, and the head of every chunk. **A stopped scan keeps what it read
//! and deletes nothing.** Committed chunks stay, and so does the part of the chunk read when the
//! cancel landed. The orphan purge doesn't run, its walk being incomplete, and the folder isn't
//! stamped as scanned. The next scan of the folder reads only the rest, the size and mtime gate
//! skipping everything already stored.

mod finish;
mod run;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use finish::Ingested;
use melodia_core::entities::scan::{ExistingTrackSummary, ScannedFile};
use melodia_core::error::{AppError, describe};
use melodia_core::utils::toast::{self, ToastKind};
use melodia_store::database::queries;
use melodia_store::media::ingest::scan_pool::ScanPool;
use melodia_store::media::ingest::scanner::{
    MediaWalk, ScanObserver, collect_media_files, scan_files_parallel, track_is_current,
};
use run::ScanRun;

/// How a scan ended, when it didn't fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanOutcome {
    Completed {
        inserted: u32,
    },
    /// Cancelled, or interrupted by shutdown, after keeping whatever it had read.
    Stopped,
}

/// Scans one folder, stopping when [`cancel`] or shutdown asks.
pub async fn scan_folder(state: &AppState, folder_id: i64) -> Result<ScanOutcome, AppError> {
    let cancel = state.scan.token();
    scan_one(state, folder_id, &cancel).await
}

/// Scans one folder in the background, tracked so shutdown waits for its last write. A failure
/// is logged and toasted, there being nobody left to hand it to.
pub fn start(state: &AppState, folder_id: i64) {
    let spawner = TaskSpawner::from_state(state);
    let state = state.clone();
    spawner.spawn(async move {
        if let Err(e) = scan_folder(&state, folder_id).await {
            log::warn!("Scan of folder {folder_id} failed: {}", describe(&e));
            toast::notify(ToastKind::OperationFailed, e.to_string());
        }
    });
}

/// Stops every scan running now. What each one has read stays in the library.
pub fn cancel(state: &AppState) {
    state.scan.cancel();
}

/// Coalesces concurrent `reconcile_watched_folders` triggers. A
/// rapid-fire kernel-overflow burst during `rsync` could otherwise spawn
/// three full library sweeps back-to-back; each one is idempotent but
/// would still re-hash every file. The flag is RAII-cleared by
/// [`ReconcileGuard`] so a panic or early-return path can't strand it.
static RECONCILE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct ReconcileGuard;
impl Drop for ReconcileGuard {
    fn drop(&mut self) {
        RECONCILE_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// Fire-and-forget background reconcile of every enabled library folder.
/// Catches files that landed (or vanished) while the watcher was off —
/// the watcher itself only reports live events, so a restart or a
/// toggle-off interval leaves DB and disk out of sync until the next
/// manual Rescan. Triggered after the watcher transitions off → on
/// (`first_launch::run`, `toggle_folder_watching(true)`) and on
/// watcher-overflow `RescanNeeded` from `file_event_processor`.
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
/// Coalesces concurrent triggers via [`RECONCILE_IN_FLIGHT`] — a second
/// call while a reconcile is mid-flight is a no-op (the in-flight pass
/// will pick up the latest disk state anyway).
pub fn reconcile_watched_folders(state: &AppState) {
    if RECONCILE_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        log::debug!("reconcile_watched_folders: already in flight, skipping");
        return;
    }
    let spawner = TaskSpawner::from_state(state);
    let state = state.clone();
    let cancel = state.scan.token();
    spawner.spawn(async move {
        let _guard = ReconcileGuard;
        let folders = match queries::folder::get_all_folders(&state.db).await {
            Ok(f) => f,
            Err(e) => {
                log::warn!("reconcile_watched_folders: load folders failed: {}", describe(&e));
                return;
            }
        };
        for folder in folders.iter().filter(|f| f.is_enabled) {
            if cancel.is_cancelled() {
                log::info!("reconcile_watched_folders: stopped, bailing between folders");
                return;
            }
            if let Err(e) = scan_one(&state, folder.id, &cancel).await {
                log::warn!(
                    "reconcile_watched_folders: scan of {} failed: {}",
                    folder.path,
                    describe(&e)
                );
            }
        }
    });
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
    cancel: &CancellationToken,
) -> Result<ScanOutcome, AppError> {
    let folder = queries::folder::get_folder_by_id(&state.db, folder_id).await?;

    let folder_path = PathBuf::from(&folder.path);
    if !folder_path.exists() {
        return Err(AppError::scanner_msg(format!("Folder does not exist: {}", folder.path)));
    }

    let run = ScanRun::start(state, cancel.clone());

    // Read-side pre-load through the read pool (before the writer tx opens):
    // size + mtime for every track already in this folder. Doesn't contend
    // with the scan's writes.
    let existing_summaries =
        queries::scan::get_existing_track_summaries_for_folder(&state.db, folder.id).await?;

    let Some(Discovery { walk, to_scan, pool }) =
        discover(folder_path, existing_summaries, Arc::clone(&run)).await?
    else {
        return Ok(ScanOutcome::Stopped);
    };
    if walk.files.is_empty() {
        return Ok(ScanOutcome::Completed { inserted: 0 });
    }

    let skipped = walk.files.len() - to_scan.len();
    if skipped > 0 {
        log::info!(
            "Incremental scan of '{}': {skipped} unchanged file(s) skipped, {} to (re)parse",
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
        let scanned_files = parse_chunk(state, chunk, &pool, Arc::clone(&run)).await?;
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
    if run.is_cancelled() {
        if ingested.any() {
            finish::commit_final(state, &folder, None, &ingested).await?;
            state.library_changed.bump();
        }
        return Ok(ScanOutcome::Stopped);
    }
    run.begin_finishing();
    finish::commit_final(state, &folder, Some(walk), &ingested).await?;
    finish::after_completed(state, folder.id).await?;
    state.library_changed.bump();

    Ok(ScanOutcome::Completed { inserted: ingested.inserted })
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
    folder_path: PathBuf,
    existing: HashMap<String, ExistingTrackSummary>,
    run: Arc<ScanRun>,
) -> Result<Option<Discovery>, AppError> {
    tokio::task::spawn_blocking(move || {
        let walk = collect_media_files(&folder_path, run.as_ref())?;
        let pool = ScanPool::for_files(walk.files.len());
        let to_scan: Vec<PathBuf> = pool.install(|| {
            walk.files
                .par_iter()
                .filter(|path| !run.is_cancelled() && !track_is_current(path, &existing))
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
