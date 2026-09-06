//! Daily auto-check loop.
//!
//! Cadence:
//!
//! - **30 s after launch**: one-shot first check (gives the runtime time
//!   to settle and avoids racing the first-launch folder scan for
//!   network I/O).
//! - **Then every 6 h**: re-arm via `tokio::time::sleep`. Not
//!   `tokio::time::interval` — `interval` fires every tick instantly
//!   after a laptop wake from sleep, which would burst-check.
//! - **24 h elapsed gate** inside each tick reads
//!   `settings.updates.last_check_unix`; if less than a day has passed
//!   the tick logs "skipped" and re-sleeps. Lets the loop survive a
//!   suspend/resume mid-cycle without spurious double-checks.
//! - **Failure backoff**: repeated failures lengthen the cadence along
//!   [`BACKOFF_LADDER`], which argues its own steps, and the next
//!   successful check resets it. Mitigates flaky-network / firewall
//!   thrash that would otherwise re-fire every 6 h.
//!
//! Per-iteration responsibilities:
//!
//! 1. Call `services::updater::check_for_update` (ETag-aware fetch +
//!    semver gate + per-platform asset resolution).
//! 2. Persist the result via `library::settings::updates::*` (success
//!    resets failures + caches `ETag`; failure increments counter).
//! 3. Push state updates into the `Updater` Slint global via
//!    `Weak<AppWindow>::upgrade_in_event_loop` so the Settings →
//!    Updates panel reflects the latest state without the user opening
//!    it manually.
//! 4. On `Available` (and not previously skipped), forward an
//!    [`UpdaterEvent::Available`] onto the `event_tx` channel so the
//!    UI-thread subscriber in `ui::settings::updater_settings` can push a toast.

use std::time::Duration;

use chrono::Utc;
use slint::{ComponentHandle, Weak};
use tokio::sync::watch;

use crate::library;
use crate::services::settings;
use crate::services::updater::{
    CheckOutcome, RELEASES_BASE, UpdaterEvent, asset_cache, check_for_update, version::is_upgrade,
};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_ui::{AppWindow, MelodiaUpdater};

const STARTUP_DELAY: Duration = Duration::from_secs(30);
const NORMAL_CADENCE: Duration = Duration::from_hours(6);
const ONE_DAY_SECS: i64 = 24 * 60 * 60;

/// Exponential backoff schedule after consecutive failures. Indexed by
/// `consecutive_failures.saturating_sub(1)` — first failure stays at
/// the normal 6h cadence, second waits 12h, third 24h, fourth+ tops
/// out at the 7d ceiling. Recovers immediately on the next successful
/// check (counter resets, `pick_next_delay` returns `NORMAL_CADENCE`).
///
/// Why exponential instead of a single 7d jump: transient failures
/// (DNS hiccup, captive portal, weekend outage) shouldn't punish the
/// user for a full week — 12h and 24h give the network time to recover
/// without thrashing 6h-after-6h-after-6h.
const BACKOFF_LADDER: &[Duration] = &[
    Duration::from_hours(12),
    Duration::from_hours(24),
    Duration::from_hours(7 * 24), // cap
];

/// Spawn the daily updater loop on the shared `TaskSpawner`. The loop
/// exits cleanly when the shutdown token fires.
///
/// `event_tx` carries notification-worthy events (the toast push that
/// fires for newly-available versions). State writes that don't need a
/// toast — `is-checking` flips, `up-to-date` repaint — go through the
/// `weak` handle directly.
pub fn spawn(
    spawner: &TaskSpawner,
    state: AppState,
    weak: Weak<AppWindow>,
    event_tx: watch::Sender<Option<UpdaterEvent>>,
) {
    spawner.spawn_cancellable(move |shutdown| async move {
        // Startup grace period — gives the first-launch scan + DB
        // pre-fetch room to settle before we add network I/O.
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(STARTUP_DELAY) => {}
        }

        loop {
            run_one_iteration(&state, &weak, &event_tx).await;

            let delay = pick_next_delay(&state);
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(delay) => {}
            }
        }
    });
}

async fn run_one_iteration(
    state: &AppState,
    weak: &Weak<AppWindow>,
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
) {
    let snapshot = match settings::read_settings(&state.paths) {
        Ok(s) => s.updates,
        Err(e) => {
            log::warn!("updater_daily: read_settings failed: {e}");
            return;
        }
    };

    if !needs_check(snapshot.last_check_unix) {
        log::info!(
            "updater_daily: check skipped — last check {}s ago (< 24h)",
            elapsed_secs(snapshot.last_check_unix)
        );
        return;
    }

    log::info!("updater_daily: checking for updates");
    set_is_checking(weak, true);

    let etag = if snapshot.last_manifest_etag.is_empty() {
        None
    } else {
        Some(snapshot.last_manifest_etag.as_str())
    };
    // `force_refresh = false`: daily checks honour the ETag so a no-op
    // 304 round-trips zero bytes and zero JSON parses. The "Check for
    // updates" button bypasses the etag — see check_for_update's doc.
    let result = check_for_update(
        state.http_client(),
        RELEASES_BASE,
        etag,
        env!("CARGO_PKG_VERSION"),
        false,
    )
    .await;
    set_is_checking(weak, false);

    let now = Utc::now();
    match result {
        Ok(outcome) => {
            handle_outcome(state, weak, event_tx, &snapshot.skipped_release, outcome, now);
        }
        Err(e) => {
            log::warn!("updater_daily: check failed: {e}");
            if let Err(persist_err) = library::settings::updates::record_check_failure(state, now) {
                log::warn!("updater_daily: record_check_failure: {persist_err}");
            }
        }
    }
}

fn handle_outcome(
    state: &AppState,
    weak: &Weak<AppWindow>,
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
    skipped_release: &str,
    outcome: CheckOutcome,
    now: chrono::DateTime<Utc>,
) {
    match outcome {
        CheckOutcome::NotModified => {
            log::info!("updater_daily: 304 Not Modified");
            // Touch last_check_unix only — keep cached version / etag intact.
            persist_success(state, now, None, None);
        }
        CheckOutcome::UpToDate => {
            log::info!("updater_daily: up to date");
            set_up_to_date(weak);
            persist_success(state, now, None, None);
        }
        CheckOutcome::NoAssetForTarget { etag } => {
            log::info!("updater_daily: manifest has no asset for current target");
            persist_success(state, now, None, etag);
        }
        CheckOutcome::UnsupportedSchema { schema, etag } => {
            // The check helper already logged the schema mismatch at
            // warn level; treat this like NoAssetForTarget — touch
            // last_check_unix + cache the etag so the next 6-hourly
            // check 304s, but don't notify (there's nothing the user
            // can act on from the in-app side).
            log::info!("updater_daily: unsupported manifest schema {schema}");
            persist_success(state, now, None, etag);
        }
        CheckOutcome::Available {
            manifest,
            asset,
            etag,
        } => {
            let version = manifest.version.clone();
            let notes_short = manifest.notes_short.clone();
            let critical = manifest.critical;
            log::info!(
                "updater_daily: update available: {version}{}",
                if critical { " (critical)" } else { "" }
            );

            // Cache the (version, asset) pair so a subsequent
            // `Updater.install` click can use it even if the
            // install-time re-fetch fails. Version is forwarded to
            // `verify_stream`'s trusted-comment cross-check.
            asset_cache::store(version.clone(), asset);

            let verdict = skip_verdict(skipped_release, &version, critical);
            if verdict.clear_skip
                && let Err(e) = library::settings::updates::reset_skipped_release(state)
            {
                log::warn!("updater_daily: reset_skipped_release: {e}");
            }

            set_update_available(weak, version.clone(), notes_short.clone(), critical);
            persist_success(state, now, Some(version.clone()), etag);

            if verdict.notify {
                let _ = event_tx.send(Some(UpdaterEvent::Available {
                    version,
                    notes_short,
                    critical,
                }));
            }
        }
    }
}

/// What the stored "skip this version" means once a manifest names a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SkipVerdict {
    /// Raise the "update available" event.
    notify: bool,
    /// Drop the stored skip — it names a version the manifest has moved past, or one semver
    /// cannot read at all.
    clear_skip: bool,
}

/// The only genuinely conditional logic in this file, and the one with a security consequence:
/// a release the publisher flagged critical must surface even where the user has muted it. Split
/// from [`handle_outcome`], which takes an `AppState` and a live window a test cannot hand it.
fn skip_verdict(skipped_release: &str, version: &str, critical: bool) -> SkipVerdict {
    let unmuted = SkipVerdict {
        notify: true,
        clear_skip: false,
    };
    if skipped_release.is_empty() {
        return unmuted;
    }

    match is_upgrade(skipped_release, version) {
        // Strictly newer than what was skipped, so the skip is spent.
        Ok(true) => SkipVerdict {
            notify: true,
            clear_skip: true,
        },
        Ok(false) => SkipVerdict {
            notify: critical,
            clear_skip: false,
        },
        Err(e) => {
            log::warn!(
                "updater_daily: stored skipped_release {skipped_release:?} not valid semver \
                 ({e}); clearing rather than muting every future notification"
            );
            SkipVerdict {
                notify: true,
                clear_skip: true,
            }
        }
    }
}

fn persist_success(
    state: &AppState,
    now: chrono::DateTime<Utc>,
    latest_version: Option<String>,
    etag: Option<String>,
) {
    if let Err(e) =
        library::settings::updates::record_check_success(state, now, latest_version, etag)
    {
        log::warn!("updater_daily: record_check_success: {e}");
    }
}

fn pick_next_delay(state: &AppState) -> Duration {
    let count = settings::read_settings(&state.paths).map_or(0, |s| s.updates.consecutive_failures);
    let delay = backoff_delay_for(count);
    if count >= 2 {
        log::info!(
            "updater_daily: {count} consecutive failures — backing off to {}h cadence",
            delay.as_secs() / 3600
        );
    }
    delay
}

/// Pure helper: maps a consecutive-failure count to the next sleep
/// duration. Extracted from [`pick_next_delay`] so the backoff ladder
/// can be unit-tested without touching settings I/O.
fn backoff_delay_for(count: u8) -> Duration {
    if count <= 1 {
        // 0 = healthy; 1 = single hiccup, stay at normal cadence.
        return NORMAL_CADENCE;
    }
    // 2nd failure → ladder[0] = 12h; 3rd → ladder[1] = 24h;
    // 4th+ → ladder[last] = 7d cap.
    let idx = (count as usize).saturating_sub(2).min(BACKOFF_LADDER.len() - 1);
    BACKOFF_LADDER[idx]
}

fn needs_check(last_check_unix: i64) -> bool {
    if last_check_unix <= 0 {
        return true;
    }
    elapsed_secs(last_check_unix) >= ONE_DAY_SECS
}

fn elapsed_secs(last_check_unix: i64) -> i64 {
    let now = Utc::now().timestamp();
    now.saturating_sub(last_check_unix)
}

fn set_is_checking(weak: &Weak<AppWindow>, on: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<MelodiaUpdater>().set_is_checking(on);
    });
}

fn set_up_to_date(weak: &Weak<AppWindow>) {
    let _ = weak.upgrade_in_event_loop(|ui| {
        let g = ui.global::<MelodiaUpdater>();
        g.set_up_to_date(true);
        g.set_update_available(false);
        g.set_error_message("".into());
    });
}

fn set_update_available(
    weak: &Weak<AppWindow>,
    version: String,
    notes_short: String,
    critical: bool,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<MelodiaUpdater>();
        g.set_up_to_date(false);
        g.set_update_available(true);
        g.set_available_version(version.into());
        g.set_notes_short(notes_short.into());
        g.set_is_critical(critical);
        g.set_error_message("".into());
    });
}

#[cfg(test)]
#[path = "tests/updater_daily_tests.rs"]
mod tests;
