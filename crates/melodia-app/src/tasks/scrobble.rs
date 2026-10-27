//! Scrobbling: a **detector** turning the view-model and position watches into scrobble and
//! now-playing decisions through the pure [`DetectorState`] and its station twin [`LiveDetector`],
//! and a **submitter** draining the durable queue to the providers with batching and backoff.
//!
//! Neither waits for a provider, so each logs its stop at debug: at info it reads as scrobbling
//! having been in use.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, SharedFlag};
use crate::tasks::TaskSpawner;
use melodia_core::entities::track::ScrobbleRow;
use melodia_core::error::describe;
use melodia_core::utils::now_unix;
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

/// Do-while shaped: each receiver's primed value is processed before the first `changed()`.
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
    let effects = detector.on_view_model(primed_vm.as_ref(), now_unix());
    process_effects(effects, &service, &db, &mut last_row).await;
    let live_effects = live.on_view_model(primed_vm.as_ref(), now_unix(), radio_scrobble.get());
    process_live_effects(live_effects, &service).await;
    let primed_tick = pos_rx.borrow_and_update().clone();
    if let Some(tick) = primed_tick {
        let effects = detector.on_position(&tick, now_unix());
        process_effects(effects, &service, &db, &mut last_row).await;
    }

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                let effects = detector.on_shutdown();
                process_effects(effects, &service, &db, &mut last_row).await;
                log::debug!("Scrobble detector stopped");
                return;
            }
            result = vm_rx.changed() => {
                if result.is_err() {
                    return; // sender dropped
                }
                let vm = vm_rx.borrow_and_update().clone();
                let effects = detector.on_view_model(vm.as_ref(), now_unix());
                process_effects(effects, &service, &db, &mut last_row).await;
                let live_effects = live.on_view_model(vm.as_ref(), now_unix(), radio_scrobble.get());
                process_live_effects(live_effects, &service).await;
            }
            result = pos_rx.changed() => {
                if result.is_err() {
                    return;
                }
                let tick = pos_rx.borrow_and_update().clone();
                if let Some(tick) = tick {
                    let effects = detector.on_position(&tick, now_unix());
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
    // With no provider ready the service drops every effect anyway, so skip the row read. The
    // detector still timed the play, so a provider connected mid-track gets its listen.
    if !service.scrobbling_active() {
        return;
    }
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

/// From the single-slot cache when it matches; `None` on a missing id or a logged query error.
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

/// Drains on entry, then parks until a new scrobble or a backoff ends. A shutdown finding it
/// parked loops once more, giving what is queued a last round under the grace.
async fn run_submitter(shutdown: CancellationToken, service: Arc<ScrobbleService>) {
    let mut backoff = BASE_BACKOFF;
    loop {
        let stop = CancellationToken::new();
        let retry = run_with_grace(service.submit_pending(&stop), &stop, &shutdown).await;
        if shutdown.is_cancelled() {
            log::debug!("Scrobble submitter stopped");
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

/// Runs a drain `round` out, cancelling `stop` once it runs [`SHUTDOWN_GRACE`] past shutdown and
/// still awaiting it: a round is a chain of requests, and dropping it would lose the writeback
/// for those already sent.
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

/// The wait to honor now and the backoff to carry. A provider's longer request wins, a 429's
/// window being a fact where the ladder is a guess; the ladder doubles regardless, so a provider
/// that keeps deferring is backed away from.
fn defer(requested: Duration, backoff: Duration) -> (Duration, Duration) {
    (requested.max(backoff), (backoff * 2).min(MAX_BACKOFF))
}

/// `None` parks until a shutdown or a new scrobble wakes the submitter.
async fn wait_for(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
#[path = "tests/scrobble_tests.rs"]
mod tests;
