//! Retires artwork the library no longer references: after a scan commits, after a folder is
//! removed, and once per launch.
//!
//! The store is content-addressed and shared, so no delete path can safely unlink a cover; this
//! retires against the whole reference set instead, which
//! [`melodia_artwork::media::image::artwork::sweep`] argues. It runs per scan because the store
//! churns: every change to a playlist's top four writes a new composite and orphans the last.

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

/// Detached, since the scan returning its count waits on none of this, and tracked, so a shutdown
/// waits for the unlinks.
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

/// Sweeps again once what the last pass spared has aged past [`GRACE`], since a library with
/// nothing to rescan may run no other sweep this session.
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

/// Retires at once the files a delete released, whatever their age. `referenced_before` is the set
/// read ahead of the delete, so every file this reaches was named by a committed row, and
/// [`GRACE`], which waits for uncommitted ones, would only hold them back.
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

/// One pass over every store, answering how many orphans the grace window spared. The renormalize
/// pass awaits it, its orphans existing only once its re-points have landed.
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

/// How long a new station logo is spared. Far shorter than [`GRACE`], which spans a scan
/// transaction: a logo's file and its cache row are one write-pool hop apart, and minutes cover
/// that hop queueing behind a scan chunk on the single-connection pool.
const RADIO_GRACE: Duration = Duration::from_mins(3);

/// The radio-logo store alone, on `tasks::radio_logo_cache`'s schedule rather than a scan's.
pub(crate) async fn run_radio_logos(db: &DbPool, paths: &Paths) -> AppResult<()> {
    sweep_stores(db, vec![("radio logo", paths.radio_logos_dir.clone())], Reach::Aged(RADIO_GRACE))
        .await
        .map(drop)
}

/// One pass over each of `stores`, answering how many orphans the grace window spared.
///
/// Listed first and the reference set read second, the order that fails safe against a scan
/// writing meanwhile: a row committed in between is visible to the query, where the reverse would
/// unlink a live cover. **One reference set for every directory**, the union of all six columns as
/// basenames, so a file is held by whichever column names it wherever it sits.
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
