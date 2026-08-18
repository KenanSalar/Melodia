//! Retires artwork the library no longer references, after a scan has committed.
//!
//! The store is content-addressed and shared, so nothing on the delete paths can safely unlink a
//! cover: eleven of twelve tracks may still point at it. Deletion happens here instead, against
//! the reference set as a whole — see [`crate::media::artwork::sweep`] for why that shape rather
//! than a refcount.
//!
//! Runs per scan rather than once at upgrade, because the store also *churns*: `compose_artwork`
//! hashes its output, so every change to a playlist's top four writes a new composite and orphans
//! the one before it.

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::{AppError, AppResult};
use crate::media::artwork::sweep::{self, GRACE, SweepReport};
use crate::services;
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Sweep both artwork stores in the background.
///
/// Detached rather than awaited: the caller is a scan returning a track count, and a directory
/// listing plus one query is maintenance nothing is blocked on. Tracked, so a shutdown landing
/// mid-sweep waits for the unlinks rather than tearing the runtime down under them.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let db = state.db.clone();
    let paths = state.paths.clone();
    spawner.spawn(async move {
        if let Err(e) = run(&db, &paths).await {
            log::warn!("Artwork sweep failed: {}", services::describe(&e));
        }
    });
}

/// One pass over both stores.
///
/// Public so the renormalize pass can await it directly rather than spawning a second task: its
/// whole output is files that *become* orphans, and only a call ordered after its re-points can
/// see them.
pub(crate) async fn run(db: &DbPool, paths: &Paths) -> AppResult<()> {
    let artwork_dir = paths.artwork_dir.clone();
    let artists_dir = paths.artists_dir.clone();

    // Listed first, and the reference set read second. Both are snapshots of state a scan is
    // concurrently writing, and this is the order that fails safe: a row committed in between is
    // visible to the query, where the reverse reads it as an orphan and unlinks a live cover.
    //
    // One `spawn_blocking` for both stores: the two listings are the same shape of work and
    // splitting them would only buy a second hop onto the same pool.
    let listed = tokio::task::spawn_blocking(move || {
        let now = std::time::SystemTime::now();
        [
            ("artwork", sweep::collect_candidates(&artwork_dir, GRACE, now)),
            ("artists", sweep::collect_candidates(&artists_dir, GRACE, now)),
        ]
    })
    .await
    .map_err(AppError::io_source)?;

    let referenced = queries::artwork::referenced_filenames(db).await?;

    let reports = tokio::task::spawn_blocking(move || {
        listed.map(|(store, (candidates, report))| {
            (store, sweep::retire(candidates, &referenced, report))
        })
    })
    .await
    .map_err(AppError::io_source)?;

    for (store, report) in reports {
        log_report(store, report);
    }
    Ok(())
}

/// Silent on a sweep that found nothing, which is the steady state once the backlog is gone.
fn log_report(store: &str, report: SweepReport) {
    if report.deleted > 0 {
        log::info!(
            "Retired {} unreferenced {store} file(s), {} KiB reclaimed ({} kept)",
            report.deleted,
            report.bytes / 1024,
            report.kept
        );
    }
    if report.failed > 0 {
        log::warn!("Could not retire {} {store} file(s); retrying next scan", report.failed);
    }
}
