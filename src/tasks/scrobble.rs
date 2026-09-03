//! Scrobbling background tasks: a **detector** that turns the player's
//! view-model + position watch channels into scrobble / now-playing decisions
//! (via the pure [`crate::services::integrations::scrobble::detector::DetectorState`]), and a
//! **submitter** that drains
//! the durable queue to the providers with per-provider batching, retry, and
//! backoff.
//!
//! Both run off the same seam OS media controls use, so nothing here touches the
//! player state machine. No `ui::*` imports — a backend failure would surface via
//! `utils::toast` if it ever needed to (routine submit failures stay silent).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::database::DbPool;
use crate::database::queries;
use crate::entities::track::ScrobbleRow;
use crate::player::state::{PlayerViewModelLight, PositionTick};
use crate::services::integrations::scrobble::detector::{DetectorState, Effect};
use crate::services::integrations::scrobble::{ScrobbleService, ScrobbleTrack};
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Base retry delay after a deferred submit; doubles up to [`MAX_BACKOFF`] on
/// repeated failure and resets to this on a clean round.
const BASE_BACKOFF: Duration = Duration::from_secs(15);

/// Ceiling on the exponential submit backoff.
const MAX_BACKOFF: Duration = Duration::from_mins(15);

/// Bound on the best-effort final flush so a shutdown can't be blocked past the
/// app's shutdown budget — the queue is already persisted regardless.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// Spawn the detector + submitter loops, both tracked for graceful shutdown.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let detector_service = state.scrobble.clone();
    let db = state.db.clone();
    let vm_rx = state.sinks.view_model.subscribe();
    let pos_rx = state.position_tx.subscribe();
    spawner.spawn_cancellable(move |shutdown| {
        run_detector(shutdown, detector_service, db, vm_rx, pos_rx)
    });

    let submitter_service = state.scrobble.clone();
    spawner.spawn_cancellable(move |shutdown| run_submitter(shutdown, submitter_service));
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Correlate the two watch channels through the pure detector, performing the DB
/// enrichment fetch + service calls each decision asks for. Do-while shaped:
/// each receiver's primed value is processed before the first `changed()`.
async fn run_detector(
    shutdown: CancellationToken,
    service: Arc<ScrobbleService>,
    db: DbPool,
    mut vm_rx: watch::Receiver<Option<PlayerViewModelLight>>,
    mut pos_rx: watch::Receiver<Option<PositionTick>>,
) {
    let mut detector = DetectorState::new();
    // Single-slot cache so a play's now-playing fetch is reused at its scrobble.
    let mut last_row: Option<(i64, ScrobbleRow)> = None;

    let primed_vm = vm_rx.borrow_and_update().clone();
    let effects = detector.on_view_model(primed_vm.as_ref(), now_ts());
    process_effects(effects, &service, &db, &mut last_row).await;
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
            Effect::Scrobble {
                track_id,
                timestamp,
            }
            | Effect::Finalize {
                track_id,
                timestamp,
            } => {
                if let Some(row) = fetch_row(db, track_id, last_row).await
                    && let Err(e) = service.enqueue_scrobble(&row, timestamp).await
                {
                    log::warn!("Failed to enqueue scrobble for track {track_id}: {e}");
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
            log::warn!("Failed to load scrobble row for track {track_id}: {e}");
            None
        }
    }
}

/// Drain the durable queue, backing off when a provider defers, and wake on a
/// new scrobble. Do-while shaped: drains once on entry, then parks.
async fn run_submitter(shutdown: CancellationToken, service: Arc<ScrobbleService>) {
    let mut backoff = BASE_BACKOFF;
    loop {
        let wait = if let Some(min) = service.submit_pending().await {
            let this_wait = min.max(backoff);
            backoff = (backoff * 2).min(MAX_BACKOFF);
            Some(this_wait)
        } else {
            backoff = BASE_BACKOFF;
            // More still queued (a >50 batch, or the other provider) — loop
            // straight into the next batch; otherwise park on a new scrobble.
            (service.queued_len() > 0).then_some(Duration::ZERO)
        };

        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                let _ = tokio::time::timeout(FLUSH_TIMEOUT, service.submit_pending()).await;
                log::info!("Scrobble submitter stopped");
                return;
            }
            () = service.notified() => {}
            () = wait_for(wait) => {}
        }
    }
}

/// Sleep for `delay` when set, else park indefinitely (only a shutdown or a new
/// scrobble wakes the submitter).
async fn wait_for(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending::<()>().await,
    }
}
