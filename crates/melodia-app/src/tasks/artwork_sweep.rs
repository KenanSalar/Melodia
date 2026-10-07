//! Retires artwork the library no longer references: after a scan has committed, after a folder
//! is removed, and once per launch.
//!
//! The store is content-addressed and shared, so nothing on the delete paths can safely unlink a
//! cover: eleven of twelve tracks may still point at it. Deletion happens here instead, against
//! the reference set as a whole — see [`melodia_artwork::media::image::artwork::sweep`] for why
//! that shape rather than a refcount.
//!
//! Runs per scan rather than once at upgrade, because the store also *churns*: `compose_artwork`
//! hashes its output, so every change to a playlist's top four writes a new composite and orphans
//! the one before it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_artwork::media::image::artwork::sweep::{self, GRACE, SweepReport};
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult, describe};
use melodia_store::database::DbPool;
use melodia_store::database::queries;

/// Sweep both artwork stores in the background.
///
/// Detached rather than awaited: the caller is a scan returning a track count, and a directory
/// listing plus one query is maintenance nothing is blocked on. Tracked, so a shutdown landing
/// mid-sweep waits for the unlinks rather than tearing the runtime down under them.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    sweep_in_background(spawner, state.db.clone(), Arc::clone(&state.paths));
}

fn sweep_in_background(spawner: &TaskSpawner, db: DbPool, paths: Arc<Paths>) {
    let follow_up = spawner.clone();
    spawner.spawn(async move {
        match run(&db, &paths).await {
            Ok(0) => {}
            Ok(_) => come_back(&follow_up, db, paths),
            Err(e) => log::warn!("Artwork sweep failed: {}", describe(&e)),
        }
    });
}

/// Set while a sweep waits out the grace window, so the sweeps of one boot reconcile, each sparing
/// the same young orphans, leave a single one waiting between them.
static COMING_BACK: AtomicBool = AtomicBool::new(false);

/// Sweeps again once what the last pass spared has aged past [`GRACE`].
///
/// Every other sweep follows a scan, and a library with nothing to rescan, or none at all, may not
/// run one for the rest of the session: a folder removed within an hour of its covers being written
/// would otherwise leave them on disk until something did.
fn come_back(spawner: &TaskSpawner, db: DbPool, paths: Arc<Paths>) {
    if COMING_BACK.swap(true, Ordering::AcqRel) {
        return;
    }
    let again = spawner.clone();
    spawner.spawn_cancellable(move |shutdown| async move {
        if shutdown.run_until_cancelled(tokio::time::sleep(GRACE)).await.is_some() {
            COMING_BACK.store(false, Ordering::Release);
            sweep_in_background(&again, db, paths);
        }
    });
}

/// Retires at once the stored files a delete released, whatever their age.
///
/// `referenced_before` is the reference set read ahead of the delete. [`GRACE`] protects a file
/// whose row hasn't committed yet, and every file this can reach was named by a committed row the
/// delete removed, so the window would only hold them back: a library removed minutes after its
/// scan kept its covers for the hour, and past it when the app closed first.
pub(crate) fn retire_released(
    spawner: &TaskSpawner,
    state: &AppState,
    referenced_before: HashSet<String>,
) {
    let db = state.db.clone();
    let paths = Arc::clone(&state.paths);
    spawner.spawn(async move {
        let reach = Reach::Released(referenced_before);
        if let Err(e) = sweep_stores(&db, all_stores(&paths), reach).await {
            log::warn!("Retiring released artwork failed: {}", describe(&e));
        }
    });
}

/// One pass over every store, answering how many orphans the grace window spared.
///
/// Public so the renormalize pass can await it directly rather than spawning a second task: its
/// whole output is files that *become* orphans, and only a call ordered after its re-points can
/// see them.
pub(crate) async fn run(db: &DbPool, paths: &Paths) -> AppResult<u32> {
    sweep_stores(db, all_stores(paths), Reach::Aged(GRACE)).await
}

fn all_stores(paths: &Paths) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("artwork", paths.artwork_dir.clone()),
        ("artists", paths.artists_dir.clone()),
        ("radio logo", paths.radio_logos_dir.clone()),
    ]
}

/// Which stored files a pass may retire.
enum Reach {
    /// Any unreferenced file older than the window.
    Aged(Duration),
    /// Only the files a delete released, named in the reference set read ahead of it, at any age.
    Released(HashSet<String>),
}

impl Reach {
    fn grace(&self) -> Duration {
        match self {
            Self::Aged(grace) => *grace,
            Self::Released(_) => Duration::ZERO,
        }
    }

    fn narrow(&self, candidates: Vec<sweep::Candidate>) -> Vec<sweep::Candidate> {
        match self {
            Self::Aged(_) => candidates,
            Self::Released(referenced_before) => sweep::named_in(candidates, referenced_before),
        }
    }
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
    sweep_stores(db, vec![("radio logo", paths.radio_logos_dir.clone())], Reach::Aged(RADIO_GRACE))
        .await
        .map(drop)
}

/// One pass over each of `stores`, answering how many orphans the grace window spared.
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
    reach: Reach,
) -> AppResult<u32> {
    let grace = reach.grace();
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
                (store, sweep::retire(reach.narrow(candidates), &referenced, report))
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(AppError::io_source)?;

    let mut deferred = 0;
    for (store, report) in reports {
        log_report(store, report);
        deferred += report.deferred;
    }
    Ok(deferred)
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

#[cfg(test)]
#[path = "tests/artwork_sweep_tests.rs"]
mod tests;
