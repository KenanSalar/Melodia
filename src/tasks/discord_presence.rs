//! Discord Rich Presence detector: turns the player's view-model watch into
//! presence updates (via the pure [`PresenceState`]) and hands them to the
//! [`DiscordPresenceService`]. Runs off the same state-change-only seam the
//! scrobbler and Material You use, so it never touches the player state machine
//! and imports no `ui::*`.
//!
//! Always spawned; inert while the feature is disabled (self-gates on
//! `service.armed()`, like `mbid_backfill`). Throttled to Discord's
//! one-update-per-15 s cap: an update landing inside the window is deferred to
//! the window's end and re-read from the (latest-only) watch, so suppressed
//! intermediates collapse into a single write on current truth — the progress
//! bar keeps animating client-side off the last anchor meanwhile.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::player::state::PlayerViewModelLight;
use crate::services::discord::DiscordPresenceService;
use crate::services::discord::model::{PresenceState, Update};
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Discord silently drops presence updates faster than one per 15 s (there is no
/// error and no client-side newest-wins queue over raw IPC), so we self-throttle.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(15);

/// Spawn the presence detector on the shared task lifecycle.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let service = state.discord.clone();
    let vm_rx = state.sinks.view_model.subscribe();
    spawner.spawn_cancellable(move |shutdown| run_detector(shutdown, service, vm_rx));
    log::info!("Discord presence task started");
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Do-while shaped like `tasks::scrobble::run_detector`: process the primed
/// view-model, then `select!` on shutdown / a change.
async fn run_detector(
    shutdown: CancellationToken,
    service: Arc<DiscordPresenceService>,
    mut vm_rx: watch::Receiver<Option<PlayerViewModelLight>>,
) {
    let mut presence = PresenceState::new();
    // False→true edge means the feature was just enabled — the card was cleared
    // while off, so the dedupe state must forget it (else the re-enable dedupes).
    let mut was_armed = false;
    // When the last update actually reached the worker — the throttle anchor.
    let mut last_update: Option<Instant> = None;

    prime(&service, &mut presence, &mut vm_rx, &mut was_armed, &mut last_update);

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                service.clear();
                log::info!("Discord presence task stopped");
                return;
            }
            changed = vm_rx.changed() => {
                if changed.is_err() {
                    return; // sender dropped
                }
            }
        }

        if !service.armed() {
            // Disabled: the settings path already cleared the card. Mark the
            // value seen, drop the throttle anchor (so a later re-enable paints
            // immediately), and reset the arming edge.
            let _ = vm_rx.borrow_and_update();
            was_armed = false;
            last_update = None;
            continue;
        }

        // Enforce the throttle by waiting out the remaining window first, so the
        // model is only ever evaluated (and its dedupe state advanced) at the
        // moment we actually send — on current truth re-read after the sleep.
        if let Some(last) = last_update {
            let elapsed = last.elapsed();
            if elapsed < MIN_UPDATE_INTERVAL {
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => {
                        service.clear();
                        log::info!("Discord presence task stopped");
                        return;
                    }
                    () = tokio::time::sleep(MIN_UPDATE_INTERVAL.saturating_sub(elapsed)) => {}
                }
            }
        }

        evaluate_and_send(
            &service,
            &mut presence,
            &mut vm_rx,
            &mut was_armed,
            &mut last_update,
        );
    }
}

/// Handle the receiver's primed value once before the loop.
fn prime(
    service: &DiscordPresenceService,
    presence: &mut PresenceState,
    vm_rx: &mut watch::Receiver<Option<PlayerViewModelLight>>,
    was_armed: &mut bool,
    last_update: &mut Option<Instant>,
) {
    if service.armed() {
        evaluate_and_send(service, presence, vm_rx, was_armed, last_update);
    } else {
        let _ = vm_rx.borrow_and_update();
    }
}

/// Read current truth from the watch, project it through the model, and push any
/// resulting update. Assumes the feature is armed. Records the send time so the
/// throttle window starts from it.
fn evaluate_and_send(
    service: &DiscordPresenceService,
    presence: &mut PresenceState,
    vm_rx: &mut watch::Receiver<Option<PlayerViewModelLight>>,
    was_armed: &mut bool,
    last_update: &mut Option<Instant>,
) {
    if !*was_armed {
        presence.reset();
        *was_armed = true;
    }
    let vm = vm_rx.borrow_and_update().clone();
    let flags = service.flags();
    let Some(update) = presence.on_view_model(vm.as_ref(), now_ts(), &flags) else {
        return; // deduped, or holding through a `loading` transition
    };
    match update {
        Update::Set(card) => service.apply(card),
        Update::Clear => service.clear(),
    }
    *last_update = Some(Instant::now());
}
