//! Folder-management API: add / remove / list / watch / scan the
//! library's source directories. Lives alongside the settings setters
//! because folders are settings-adjacent (the `Folder` table backs
//! `Settings → Library → Music Folders`) and shares the same
//! validation-then-persist cadence.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::database::queries;
use crate::entities::folder;
use crate::error::AppError;
use crate::media::scanner::{collect_media_files, scan_files_parallel, track_is_current};
use crate::services;
use crate::state::{AppState, ScanProgressTick};
use crate::tasks::{self, TaskSpawner};

/// Validates a new folder path against existing folders.
/// Returns IDs of existing child folders that should be removed (covered by the new parent).
fn validate_folder_path(
    new_path: &Path,
    existing_folders: &[folder::Folder],
) -> Result<Vec<i64>, AppError> {
    if !new_path.exists() {
        return Err(AppError::Validation(format!(
            "Path does not exist: {}",
            new_path.display()
        )));
    }
    if !new_path.is_dir() {
        return Err(AppError::Validation(format!(
            "Path is not a directory: {}",
            new_path.display()
        )));
    }

    let canonical_new = crate::utils::canonicalize_path(new_path).map_err(|e| {
        AppError::Validation(format!(
            "Cannot resolve path {}: {}",
            new_path.display(),
            e
        ))
    })?;

    let mut children_to_remove = Vec::new();

    for folder in existing_folders {
        let existing_path = Path::new(&folder.path);
        let Ok(canonical_existing) = crate::utils::canonicalize_path(existing_path) else {
            continue;
        };

        if canonical_new == canonical_existing {
            return Err(AppError::Validation(format!(
                "This folder is already in your library: {}",
                folder.path
            )));
        }

        if canonical_new.starts_with(&canonical_existing) {
            return Err(AppError::Validation(format!(
                "This folder is already covered by: {}",
                folder.path
            )));
        }

        if canonical_existing.starts_with(&canonical_new) {
            children_to_remove.push(folder.id);
        }
    }

    Ok(children_to_remove)
}

pub async fn add_folder(state: &AppState, path: String) -> Result<folder::Folder, AppError> {
    let new_path = Path::new(&path);
    let existing_folders = queries::folder::get_all_folders(&state.db).await?;
    let children_to_remove = validate_folder_path(new_path, &existing_folders)?;

    queries::folder::delete_folders_by_ids(&state.db, &children_to_remove).await?;

    let canonical = crate::utils::canonicalize_path(new_path)
        .map_err(|e| AppError::Validation(format!("Cannot resolve path: {e}")))?;
    let canonical_str = canonical.to_string_lossy().into_owned();

    let folder = queries::folder::insert_folder(&state.db, &canonical_str, true).await?;

    // Notify subscribers (Tracks view, folder list) — the new folder row is
    // visible immediately, and any child folders that were auto-aggregated
    // away cascade-deleted their tracks. The subsequent scan will fire its
    // own bump on completion.
    state
        .library_changed_tx
        .send_modify(|n| *n = n.wrapping_add(1));

    Ok(folder)
}

pub async fn remove_folder(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::folder::delete_folder(&state.db, id).await?;
    // Cascade-delete removes every track in this folder; subscribers (Tracks
    // view + folder list) need to re-fetch or the UI keeps the stale rows.
    state
        .library_changed_tx
        .send_modify(|n| *n = n.wrapping_add(1));
    Ok(())
}

pub async fn get_folders(state: &AppState) -> Result<Vec<folder::Folder>, AppError> {
    queries::folder::get_all_folders(&state.db).await
}

pub async fn toggle_folder_watching(state: &AppState, enabled: bool) -> Result<(), AppError> {
    if enabled {
        // Resolve folder paths via the async DB query *before* taking the
        // (sync) parking_lot lock — the guard must not span an await point.
        let folders = queries::folder::get_all_folders(&state.db).await?;
        let paths: Vec<PathBuf> = folders
            .iter()
            .filter(|f| f.is_enabled)
            .map(|f| PathBuf::from(&f.path))
            .collect();
        {
            let mut watcher = state.watcher.lock();
            watcher.start(&paths)?;
        }
        // Catch files added / removed while the watcher was off — the
        // watcher itself only reports live events.
        reconcile_watched_folders(state);
    } else {
        let mut watcher = state.watcher.lock();
        watcher.stop();
    }
    Ok(())
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
/// Sequential per folder: `SQLite` has a single writer, and
/// `scan_progress_tx` is a `watch` channel that parallel scans would
/// clobber. Each `scan_folder_internal` already bumps
/// `library_changed_tx`, so Browse/Tracks views auto-refresh per folder
/// as scans complete.
///
/// Tracked on `TaskSpawner` so shutdown waits for the in-flight folder's
/// transaction to commit rather than tearing the runtime out from under
/// a partial scan; shutdown between folders bails early since each
/// folder is atomic on its own.
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
    spawner.spawn_cancellable(move |shutdown| async move {
        let _guard = ReconcileGuard;
        let folders = match queries::folder::get_all_folders(&state.db).await {
            Ok(f) => f,
            Err(e) => {
                log::warn!("reconcile_watched_folders: load folders failed: {e}");
                return;
            }
        };
        for folder in folders.iter().filter(|f| f.is_enabled) {
            if shutdown.is_cancelled() {
                log::info!("reconcile_watched_folders: shutdown requested, bailing between folders");
                return;
            }
            if let Err(e) = scan_folder_internal(&state, folder.id).await {
                log::warn!(
                    "reconcile_watched_folders: scan of {} failed: {}",
                    folder.path, e
                );
            }
        }
    });
}

/// Persist the `folder_watching_enabled` flag *first*, then flip the watcher.
/// Persist-first means a `start()` failure leaves disk consistent with the
/// user's intent — `tasks::first_launch::run` will retry on next launch
/// from the persisted flag. The reverse order would leave a running watcher
/// that doesn't restart next session if `mutate_settings` failed.
pub async fn set_folder_watching_enabled(
    state: &AppState,
    enabled: bool,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.library.folder_watching_enabled = enabled;
    })?;
    toggle_folder_watching(state, enabled).await
}

pub async fn scan_folder(state: &AppState, folder_id: i64) -> Result<u32, AppError> {
    scan_folder_internal(state, folder_id).await
}

/// Scan-delta size above which the denormalized-stats triggers are dropped
/// for the ingest and rebuilt via one `recalculate_all_stats` sweep at the
/// end. At or below it the triggers stay enabled: per-row maintenance on a
/// handful of inserts/updates/deletes is far cheaper than the full 3-table
/// correlated-subquery recalc the drop would force. Same value and
/// rationale as the watcher reconcile path's `BULK_THRESHOLD`
/// (`tasks/file_event_processor/reconcile.rs`).
const SCAN_BULK_THRESHOLD: usize = 20;

/// Files per ingest write-transaction on the bulk scan path. Large enough
/// that per-chunk overhead (begin/commit + the 6 trigger DDL statements)
/// is noise, small enough that interactive writes waiting on the single
/// writer connection get a slot every few seconds even on slow disks.
const TX_CHUNK_FILES: usize = 2_000;

/// Internal scan implementation. Reusable by `scan_folder` and the first-launch task.
pub async fn scan_folder_internal(
    state: &AppState,
    folder_id: i64,
) -> Result<u32, AppError> {
    let folder = queries::folder::get_folder_by_id(&state.db, folder_id).await?;

    let folder_path = Path::new(&folder.path);
    if !folder_path.exists() {
        return Err(AppError::scanner_msg(format!(
            "Folder does not exist: {}",
            folder.path
        )));
    }

    // Read-side pre-load through the read pool (before the writer tx opens):
    // size + mtime for every track already in this folder. Doesn't contend
    // with the scan's writes.
    let existing_summaries =
        queries::scan::get_existing_track_summaries_for_folder(&state.db, folder.id).await?;

    // Directory walk + incremental filter on the blocking pool. Both are
    // synchronous syscall loops (WalkDir over the whole tree, then one
    // `fs::metadata` per already-known file inside `track_is_current`) —
    // running them inline in this async fn would pin one of the runtime's
    // few workers for the duration on a cold-cache disk, stalling position
    // ticks and watcher deliveries during boot reconciles.
    //
    // The incremental filter only (re)parses files that are new, or whose
    // size or mtime no longer matches the stored row. Byte-unchanged files
    // keep their existing DB metadata untouched — Lofty is skipped for
    // them entirely, which is the bulk of a typical startup rescan.
    let folder_path_owned = folder_path.to_path_buf();
    let (files, to_scan) = tokio::task::spawn_blocking(move || {
        let files = collect_media_files(&folder_path_owned);
        let to_scan: Vec<PathBuf> = files
            .iter()
            .filter(|path| !track_is_current(path, &existing_summaries))
            .cloned()
            .collect();
        (files, to_scan)
    })
    .await
    .map_err(|e| AppError::scanner("Scan walk task failed", e))?;

    if files.is_empty() {
        return Ok(0);
    }

    // Drop guard: any return path (including `?` early-exits below) clears
    // the scan-progress channel so the UI doesn't keep a stale progress bar.
    let _scan_guard = ScanProgressGuard(&state.scan_progress_tx);

    let skipped = files.len() - to_scan.len();
    if skipped > 0 {
        log::info!(
            "Incremental scan of '{}': {skipped} unchanged file(s) skipped, {} to (re)parse",
            folder.path,
            to_scan.len()
        );
    }
    let total = u32::try_from(to_scan.len()).unwrap_or(u32::MAX);

    // Decided before `to_scan` moves into the scan task. Edge: a tiny
    // `to_scan` combined with a huge orphan purge (folder emptied
    // externally) runs the delete trigger per orphaned row — rare, still
    // correct, and accepted over plumbing the orphan count (unknown until
    // inside the transaction) into this decision.
    let is_bulk = to_scan.len() > SCAN_BULK_THRESHOLD;

    // Publish an initial 0-of-total tick so the UI shows the bar immediately.
    let _ = state.scan_progress_tx.send(Some(ScanProgressTick {
        folder_id: folder.id,
        scanned: 0,
        total,
        current_file: String::new(),
    }));

    let scanned_files = if to_scan.is_empty() {
        Vec::new()
    } else {
        let artwork_dir = state.paths.artwork_dir.clone();
        let cover_cache_clone = state.cover_cache.clone();
        let progress_tx = state.scan_progress_tx.clone();
        let progress_folder_id = folder.id;
        // Throttle progress publishes to ~20 Hz. The scanner already gates
        // at every-10-files, but a fast SSD scan can fire those gates much
        // faster than the UI can paint — and each tick allocates a String
        // (filename) for the watch channel. Always publish the final tick
        // (`scanned == total`) so the UI knows the scan finished.
        let last_send =
            std::sync::Arc::new(parking_lot::Mutex::new(std::time::Instant::now()));
        tokio::task::spawn_blocking(move || {
            scan_files_parallel(
                &to_scan,
                &artwork_dir,
                &cover_cache_clone,
                &move |scanned, file_name| {
                    let is_final = scanned == total;
                    if !is_final {
                        let mut last = last_send.lock();
                        if last.elapsed() < std::time::Duration::from_millis(50) {
                            return;
                        }
                        *last = std::time::Instant::now();
                    }
                    let _ = progress_tx.send(Some(ScanProgressTick {
                        folder_id: progress_folder_id,
                        scanned,
                        total,
                        current_file: file_name.to_owned(),
                    }));
                },
            )
        })
        .await
        .map_err(|e| AppError::scanner("Scan task failed", e))?
    };

    let scan_timestamp = crate::utils::now_rfc3339();

    // --- Phase 1: ingest, chunked into separate write transactions on the
    // bulk path. The single writer connection frees between chunks, so
    // interactive writes (favorite toggles, play-count flushes, position
    // saves) no longer queue behind a multi-minute first scan. Each chunk
    // is self-consistent: stats triggers are dropped and recreated INSIDE
    // its transaction, so a crash never leaves them missing — the stats
    // merely lag until the final recalc below, which is invisible to the
    // UI because `library_changed_tx` is bumped only after the final
    // commit. A crash between chunks leaves committed tracks behind; the
    // next scan's size+mtime gate makes the re-run a cheap no-op over them.
    let mut inserted_count: u32 = 0;
    let mut moved_count: u32 = 0;
    let mut updated_count: u32 = 0;
    let chunk_size = if is_bulk {
        TX_CHUNK_FILES
    } else {
        scanned_files.len().max(1)
    };
    for chunk in scanned_files.chunks(chunk_size) {
        let mut tx = state.db.write().begin().await?;
        if is_bulk {
            queries::stats::disable_stats_triggers(&mut tx).await?;
        }
        let result = queries::ingest::ingest_scanned_files(
            &mut tx,
            chunk,
            &queries::FolderResolution::Fixed(folder.id),
            &scan_timestamp,
            true,
        )
        .await?;
        if is_bulk {
            queries::stats::enable_stats_triggers(&mut tx).await?;
        }
        tx.commit().await?;
        inserted_count += result.inserted_count;
        moved_count += result.moved_count;
        updated_count += result.updated_count;
    }

    // --- Phase 2: orphan pruning + album-artwork roll-up + stats recalc
    // in one final transaction.
    let mut tx = state.db.write().begin().await?;
    let all_db_paths = queries::scan::get_all_track_paths_for_folder(&mut tx, folder.id).await?;
    // Orphans = DB rows whose file is no longer on disk. Compare against the
    // full on-disk set (`files`), NOT `scanned_files`: with the incremental
    // filter above, `scanned_files` omits unchanged files that are still
    // present, and treating those as orphans would delete the whole library.
    let on_disk_paths: HashSet<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let orphans: Vec<String> = all_db_paths
        .into_iter()
        .filter(|p| !on_disk_paths.contains(p))
        .collect();
    // On the bulk path a full recalc follows anyway, so a large orphan
    // purge shouldn't pay per-row delete triggers on top — drop them for
    // the delete. Small deltas keep the triggers on (per-row maintenance
    // is the whole point of the `!is_bulk` branch).
    let bulk_orphan_purge = is_bulk && !orphans.is_empty();
    if bulk_orphan_purge {
        queries::stats::disable_stats_triggers(&mut tx).await?;
    }
    if !orphans.is_empty() {
        log::info!(
            "Removing {} orphaned tracks from folder {}",
            orphans.len(),
            folder.path
        );
        queries::scan::delete_tracks_by_paths_batch(&mut tx, &orphans).await?;
    }

    // No-op rescans (every file unchanged, no orphans, no inserts) skip the
    // album-artwork propagation and the full stats recalc — both are O(rows)
    // sweeps that produce identical values when nothing changed.
    let any_changes =
        inserted_count > 0 || updated_count > 0 || moved_count > 0 || !orphans.is_empty();

    if any_changes {
        queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
    }
    // Small deltas (`!is_bulk`) never dropped the triggers, so per-row
    // maintenance already kept the stats correct — no recalc needed at all.
    if is_bulk {
        if any_changes {
            queries::stats::recalculate_all_stats(&mut tx).await?;
        }
        if bulk_orphan_purge {
            queries::stats::enable_stats_triggers(&mut tx).await?;
        }
    }
    tx.commit().await?;

    if moved_count > 0 {
        log::info!("Detected {moved_count} moved/renamed files");
    }
    if updated_count > 0 {
        log::info!("Updated metadata for {updated_count} changed files");
    }

    let now = crate::utils::now_rfc3339();
    queries::folder::update_folder_last_scanned(&state.db, folder_id, &now).await?;

    services::artist_images::spawn_fetch(
        state.paths.clone(),
        state.db.clone(),
        state.http_client().clone(),
    );
    tasks::retroactive_hash::spawn(&TaskSpawner::from_state(state), state);

    state
        .library_changed_tx
        .send_modify(|n| *n = n.wrapping_add(1));

    Ok(inserted_count)
}

/// RAII helper: clears `scan_progress_tx` on drop so any early-return path
/// inside `scan_folder_internal` removes the UI's progress indicator.
struct ScanProgressGuard<'a>(&'a tokio::sync::watch::Sender<Option<ScanProgressTick>>);

impl Drop for ScanProgressGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.send(None);
    }
}

#[cfg(test)]
#[path = "tests/folders_tests.rs"]
mod tests;
