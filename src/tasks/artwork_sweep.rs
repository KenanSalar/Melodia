//! Retires artwork the library no longer references, after a scan has committed.
//!
//! The store is content-addressed and shared, so nothing on the delete paths can safely unlink a
//! cover: eleven of twelve tracks may still point at it. Deletion happens here instead, against
//! the reference set as a whole — see [`crate::media::image::artwork::sweep`] for why that shape rather
//! than a refcount.
//!
//! Runs per scan rather than once at upgrade, because the store also *churns*: `compose_artwork`
//! hashes its output, so every change to a playlist's top four writes a new composite and orphans
//! the one before it.

use std::path::PathBuf;
use std::time::Duration;

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::{AppError, AppResult, describe};
use crate::media::image::artwork::sweep::{self, GRACE, SweepReport};
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
            log::warn!("Artwork sweep failed: {}", describe(&e));
        }
    });
}

/// One pass over both stores.
///
/// Public so the renormalize pass can await it directly rather than spawning a second task: its
/// whole output is files that *become* orphans, and only a call ordered after its re-points can
/// see them.
pub(crate) async fn run(db: &DbPool, paths: &Paths) -> AppResult<()> {
    sweep_stores(
        db,
        vec![
            ("artwork", paths.artwork_dir.clone()),
            ("artists", paths.artists_dir.clone()),
            ("radio logo", paths.radio_logos_dir.clone()),
        ],
        GRACE,
    )
    .await
}

/// How long a station logo is protected from the sweep purely for being new.
///
/// Far shorter than [`GRACE`], because the window it covers is a different size. That one protects
/// a cover a scan worker wrote before its transaction committed, which is as long as the
/// transaction; a logo's file and its cache row are written by the same task, one write-pool hop
/// apart. What sets the floor is that the pool is single-connection, so the hop can queue behind a
/// scan chunk — minutes of headroom over a gap measured in milliseconds. Inheriting the hour meant
/// a store the retention pass had just released stayed on disk for the rest of the session.
const RADIO_GRACE: Duration = Duration::from_mins(3);

/// Sweep the radio-logo store alone.
///
/// **Its own entry point because its own schedule is the point.** Everything else here is retired
/// after a *scan*, which is the only thing that orphans a cover — and which a user who browses
/// radio and never touches their music folders may not run for weeks, leaving every logo dropped
/// by the retention pass sitting on disk until they do. One directory rather than three keeps that
/// cheap enough to run whenever Radio is done with.
pub(crate) async fn run_radio_logos(db: &DbPool, paths: &Paths) -> AppResult<()> {
    sweep_stores(db, vec![("radio logo", paths.radio_logos_dir.clone())], RADIO_GRACE).await
}

/// One pass over each of `stores`.
///
/// Listed first, and the reference set read second. Both are snapshots of state a scan is
/// concurrently writing, and this is the order that fails safe: a row committed in between is
/// visible to the query, where the reverse reads it as an orphan and unlinks a live cover.
///
/// One `spawn_blocking` for every store: the listings are the same shape of work and splitting
/// them would only buy more hops onto the same pool.
///
/// **However many directories, one reference set**, which is what lets a store move without the
/// query moving with it: the set is the union of all six artwork columns reduced to basenames, so
/// a radio logo is held alive by `radio_stations.artwork_path` or by its cache row wherever it
/// happens to sit. That is also why the logos that predate their own directory are safe where
/// they are.
async fn sweep_stores(
    db: &DbPool,
    stores: Vec<(&'static str, PathBuf)>,
    grace: Duration,
) -> AppResult<()> {
    let listed = tokio::task::spawn_blocking(move || {
        let now = std::time::SystemTime::now();
        stores
            .into_iter()
            .map(|(store, dir)| (store, sweep::collect_candidates(&dir, grace, now)))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(AppError::io_source)?;

    let referenced = queries::artwork::referenced_filenames(db).await?;

    let reports = tokio::task::spawn_blocking(move || {
        listed
            .into_iter()
            .map(|(store, (candidates, report))| {
                (store, sweep::retire(candidates, &referenced, report))
            })
            .collect::<Vec<_>>()
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
