//! Daily auto-check loop: first 30 s after launch, clear of the launch scan, then every 6 h on a
//! `sleep` rather than an `interval`, which fires every missed tick at once after a laptop wakes.
//!
//! The auto-check switch is read per tick, since the welcome card and Settings both offer it
//! mid-session, and toggling it cuts the sleep short (`AppState::auto_check_changed`). A 24 h gate
//! on `last_check_unix` keeps a resume mid-cycle from checking twice, and repeated failures back
//! off along [`BACKOFF_LADDER`]. A failure is logged and nothing more: a background check stays
//! quiet.

use std::time::Duration;

use tokio::sync::watch;

use crate::services::settings;
use crate::services::updater::{PanelPaint, UpdaterEvent, run_check};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_core::utils::now_unix;

const STARTUP_DELAY: Duration = Duration::from_secs(30);
const NORMAL_CADENCE: Duration = Duration::from_hours(6);
const ONE_DAY_SECS: i64 = 24 * 60 * 60;

/// From the second consecutive failure on, a single hiccup keeping the normal cadence, and reset by
/// the next success. Stepped rather than one jump to the cap, so a weekend outage doesn't cost a
/// week.
const BACKOFF_LADDER: &[Duration] = &[
    Duration::from_hours(12),
    Duration::from_hours(24),
    Duration::from_hours(7 * 24), // cap
];

/// `event_tx` carries what earns a toast; `paint` the panel writes that don't.
pub fn spawn(
    spawner: &TaskSpawner,
    state: AppState,
    paint: impl Fn(PanelPaint) + Send + Sync + 'static,
    event_tx: watch::Sender<Option<UpdaterEvent>>,
) {
    let mut auto_check = state.auto_check_changed.subscribe();

    spawner.spawn_cancellable(move |shutdown| async move {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(STARTUP_DELAY) => {}
        }

        loop {
            run_one_iteration(&state, &paint, &event_tx).await;

            let delay = pick_next_delay(&state);
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                // Only cuts a sleep the switch has outlived: the iteration re-reads the file, and
                // `needs_check`'s 24 h floor, stamped by a failed check too, keeps a bounced
                // switch from hammering.
                woken = auto_check.changed() => {
                    if woken.is_err() {
                        return;
                    }
                }
                () = tokio::time::sleep(delay) => {}
            }
        }
    });
}

async fn run_one_iteration(
    state: &AppState,
    paint: &impl Fn(PanelPaint),
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
) {
    let snapshot = match settings::read_settings(&state.paths) {
        Ok(s) => s.updates,
        Err(e) => {
            log::warn!("updater_daily: read_settings failed: {}", describe(&e));
            return;
        }
    };

    // Ahead of the elapsed gate: a disabled check owes no log line every 6 h either.
    if !snapshot.auto_check_enabled {
        return;
    }

    let now = now_unix();
    if !needs_check(snapshot.last_check_unix, now) {
        log::info!(
            "updater_daily: check skipped — last check {}s ago (< 24h)",
            elapsed_secs(snapshot.last_check_unix, now)
        );
        return;
    }

    log::info!("updater_daily: checking for updates");
    paint(PanelPaint::CheckStarted);
    let checked = run_check(state.http_client(), &state.paths).await;
    paint(PanelPaint::CheckEnded);

    match checked {
        Ok(finding) => finding.deliver(paint, event_tx),
        Err(e) => log::warn!("updater_daily: check failed: {}", describe(&e)),
    }
}

fn pick_next_delay(state: &AppState) -> Duration {
    // A disabled loop records no failures, so its ladder means nothing. This covers a
    // `settings.json` edited underneath, which bumps nothing and would wait out a stale 7 d step.
    let count = settings::read_settings(&state.paths)
        .ok()
        .filter(|s| s.updates.auto_check_enabled)
        .map_or(0, |s| s.updates.consecutive_failures);
    let delay = backoff_delay_for(count);
    if count >= 2 {
        log::info!(
            "updater_daily: {count} consecutive failures — backing off to {}h cadence",
            delay.as_secs() / 3600
        );
    }
    delay
}

/// Split from [`pick_next_delay`] so the ladder is testable without settings I/O.
fn backoff_delay_for(count: u8) -> Duration {
    if count <= 1 {
        return NORMAL_CADENCE;
    }
    let idx = (count as usize).saturating_sub(2).min(BACKOFF_LADDER.len() - 1);
    BACKOFF_LADDER[idx]
}

/// Whether a day has passed since `last_check_unix` at `now`, both Unix seconds. A stamp ahead of
/// `now` is a clock stepped back, and waits rather than checking again.
fn needs_check(last_check_unix: i64, now: i64) -> bool {
    if last_check_unix <= 0 {
        return true;
    }
    elapsed_secs(last_check_unix, now) >= ONE_DAY_SECS
}

fn elapsed_secs(last_check_unix: i64, now: i64) -> i64 {
    now.saturating_sub(last_check_unix)
}

#[cfg(test)]
#[path = "tests/updater_daily_tests.rs"]
mod tests;
