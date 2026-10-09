//! Scrobbling background tasks: a **detector** that turns the player's
//! view-model + position watch channels into scrobble / now-playing decisions
//! (via the pure
//! [`melodia_integrations::services::integrations::scrobble::detector::DetectorState`], and
//! its station twin `live_detector::LiveDetector`),
//! and a **submitter** that drains the durable queue to the providers with
//! per-provider batching, retry, and backoff.
//!
//! Both run off the same seam OS media controls use, so nothing here touches the
//! player state machine. No `ui::*` imports — a backend failure would surface via
//! `utils::toast` if it ever needed to (routine submit failures stay silent).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, SharedFlag};
use crate::tasks::TaskSpawner;
use melodia_core::entities::track::ScrobbleRow;
use melodia_core::error::describe;
use melodia_engine::player::engine::state::{PlayerViewModelLight, PositionTick};
use melodia_integrations::services::integrations::scrobble::detector::{DetectorState, Effect};
use melodia_integrations::services::integrations::scrobble::live_detector::{
    LiveDetector, LiveEffect,
};
use melodia_integrations::services::integrations::scrobble::{ScrobbleService, ScrobbleTrack};
use melodia_store::database::DbPool;
use melodia_store::database::queries;

/// Base retry delay after a deferred submit; doubles up to [`MAX_BACKOFF`] on
/// repeated failure and resets to this on a clean round.
const BASE_BACKOFF: Duration = Duration::from_secs(15);

/// Ceiling on the exponential submit backoff.
const MAX_BACKOFF: Duration = Duration::from_mins(15);

/// How long a drain may keep sending once shutdown fires, before it is stopped and
/// writes back what went out. Has to sit inside `shutdown::flush_tasks_and_db`'s
/// budget; whatever is left is persisted and goes out on the next launch.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Spawn the detector + submitter loops, both tracked for graceful shutdown.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let detector_service = state.scrobble.clone();
    let db = state.db.clone();
    let radio_scrobble = state.radio_scrobble.clone();
    let vm_rx = state.sinks.view_model.subscribe();
    let pos_rx = state.position_tx.subscribe();
    spawner.spawn_cancellable(move |shutdown| {
        run_detector(shutdown, detector_service, db, radio_scrobble, vm_rx, pos_rx)
    });

    let submitter_service = state.scrobble.clone();
    spawner.spawn_cancellable(move |shutdown| run_submitter(shutdown, submitter_service));
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Correlate the two watch channels through the pure detectors, performing the DB
/// enrichment fetch + service calls each decision asks for. Do-while shaped:
/// each receiver's primed value is processed before the first `changed()`.
async fn run_detector(
    shutdown: CancellationToken,
    service: Arc<ScrobbleService>,
    db: DbPool,
    radio_scrobble: SharedFlag,
    mut vm_rx: watch::Receiver<Option<PlayerViewModelLight>>,
    mut pos_rx: watch::Receiver<Option<PositionTick>>,
) {
    let mut detector = DetectorState::new();
    let mut live = LiveDetector::new();
    // Single-slot cache so a play's now-playing fetch is reused at its scrobble.
    let mut last_row: Option<(i64, ScrobbleRow)> = None;

    let primed_vm = vm_rx.borrow_and_update().clone();
    let effects = detector.on_view_model(primed_vm.as_ref(), now_ts());
    process_effects(effects, &service, &db, &mut last_row).await;
    let live_effects = live.on_view_model(primed_vm.as_ref(), now_ts(), radio_scrobble.get());
    process_live_effects(live_effects, &service).await;
    let primed_tick = pos_rx.borrow_and_update().clone();
    if let Some(tick) = primed_tick {
        let effects = detector.on_position(&tick, now_ts());
        process_effects(effects, &service, &db, &mut last_row).await;
    }

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                let effects = detector.on_shutdown();
                process_effects(effects, &service, &db, &mut last_row).await;
                log::info!("Scrobble detector stopped");
                return;
            }
            result = vm_rx.changed() => {
                if result.is_err() {
                    return; // sender dropped
                }
                let vm = vm_rx.borrow_and_update().clone();
                let effects = detector.on_view_model(vm.as_ref(), now_ts());
                process_effects(effects, &service, &db, &mut last_row).await;
                let live_effects = live.on_view_model(vm.as_ref(), now_ts(), radio_scrobble.get());
                process_live_effects(live_effects, &service).await;
            }
            result = pos_rx.changed() => {
                if result.is_err() {
                    return;
                }
                let tick = pos_rx.borrow_and_update().clone();
                if let Some(tick) = tick {
                    let effects = detector.on_position(&tick, now_ts());
                    process_effects(effects, &service, &db, &mut last_row).await;
                }
            }
        }
    }
}

async fn process_effects(
    effects: Vec<Effect>,
    service: &ScrobbleService,
    db: &DbPool,
    last_row: &mut Option<(i64, ScrobbleRow)>,
) {
    for effect in effects {
        match effect {
            Effect::NowPlaying { track_id } => {
                if let Some(row) = fetch_row(db, track_id, last_row).await
                    && let Some(track) = ScrobbleTrack::from_row(&row)
                {
                    service.update_now_playing(track);
                }
            }
            Effect::Scrobble { track_id, timestamp } | Effect::Finalize { track_id, timestamp } => {
                if let Some(row) = fetch_row(db, track_id, last_row).await
                    && let Err(e) = service.enqueue_scrobble(&row, timestamp).await
                {
                    log::warn!("Failed to enqueue scrobble for track {track_id}: {}", describe(&e));
                }
            }
        }
    }
}

/// A station's song already carries its whole payload, so there is no row to fetch.
async fn process_live_effects(effects: Vec<LiveEffect>, service: &ScrobbleService) {
    for effect in effects {
        match effect {
            LiveEffect::NowPlaying(track) => service.update_now_playing(track),
            LiveEffect::Scrobble { track, timestamp } => {
                if let Err(e) = service.enqueue_track(track, timestamp).await {
                    log::warn!("Failed to enqueue a radio scrobble: {}", describe(&e));
                }
            }
        }
    }
}

/// The row for `track_id`, from the single-slot cache when it matches, else
/// fetched and cached. `None` on a missing id or a query error (logged).
async fn fetch_row(
    db: &DbPool,
    track_id: i64,
    last_row: &mut Option<(i64, ScrobbleRow)>,
) -> Option<ScrobbleRow> {
    if let Some((id, row)) = last_row.as_ref()
        && *id == track_id
    {
        return Some(row.clone());
    }
    match queries::track::get_scrobble_row(db, track_id).await {
        Ok(Some(row)) => {
            *last_row = Some((track_id, row.clone()));
            Some(row)
        }
        Ok(None) => None,
        Err(e) => {
            log::warn!("Failed to load scrobble row for track {track_id}: {}", describe(&e));
            None
        }
    }
}

/// Drain the durable queue, backing off when a provider defers, and wake on a
/// new scrobble. Do-while shaped: drains once on entry, then parks. A shutdown
/// that finds it parked loops once more, so what is still queued gets a last
/// round under the grace.
async fn run_submitter(shutdown: CancellationToken, service: Arc<ScrobbleService>) {
    let mut backoff = BASE_BACKOFF;
    loop {
        let stop = CancellationToken::new();
        let retry = run_with_grace(service.submit_pending(&stop), &stop, &shutdown).await;
        if shutdown.is_cancelled() {
            log::info!("Scrobble submitter stopped");
            return;
        }

        let wait = if let Some(min) = retry {
            let (this_wait, next) = defer(min, backoff);
            backoff = next;
            Some(this_wait)
        } else {
            backoff = BASE_BACKOFF;
            // More still queued (a >50 batch, or the other provider) — loop
            // straight into the next batch; otherwise park on a new scrobble.
            (service.queued_len() > 0).then_some(Duration::ZERO)
        };

        tokio::select! {
            biased;
            () = shutdown.cancelled() => {}
            () = service.notified() => {}
            () = wait_for(wait) => {}
        }
    }
}

/// Runs a drain `round` out, unless it is still going [`SHUTDOWN_GRACE`] after shutdown;
/// then `stop`, which the round has to be watching, is cancelled and the round awaited
/// rather than dropped. A round is a chain of sequential requests, each allowed the HTTP
/// client's whole read timeout, and dropping it would lose the writeback for every one
/// that already landed.
async fn run_with_grace<T>(
    round: impl Future<Output = T>,
    stop: &CancellationToken,
    shutdown: &CancellationToken,
) -> T {
    tokio::pin!(round);

    let grace_spent = async {
        shutdown.cancelled().await;
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    };
    tokio::select! {
        biased;
        finished = &mut round => return finished,
        () = grace_spent => stop.cancel(),
    }
    round.await
}

/// What a deferral earns: the wait to honor now, and the backoff to carry into the next one.
///
/// The provider's own request wins when it named a longer one, since a 429's window is a fact
/// about the server and the local ladder is only a guess. The ladder doubles regardless, so a
/// provider that keeps deferring without naming a wait is backed away from rather than polled
/// at a fixed rate.
fn defer(requested: Duration, backoff: Duration) -> (Duration, Duration) {
    (requested.max(backoff), (backoff * 2).min(MAX_BACKOFF))
}

/// Sleep for `delay` when set, else park indefinitely (only a shutdown or a new
/// scrobble wakes the submitter).
async fn wait_for(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
#[path = "tests/scrobble_tests.rs"]
mod tests;
