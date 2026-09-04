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
//! - **Failure backoff**: after 3 consecutive failed checks, swap the
//!   6 h cadence for 7 d. Resets to 6 h on the next successful check
//!   (mitigates flaky-network / firewall thrash that would otherwise
//!   re-fire every 6 h).
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
    CheckOutcome, UpdaterEvent, asset_cache, check_for_update, version::is_upgrade,
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
    let result =
        check_for_update(state.http_client(), etag, env!("CARGO_PKG_VERSION"), false).await;
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

            // Clear any stale skip that's now older than the live
            // manifest's version. After this, the toast/notification
            // path uses the (possibly-cleared) skip slot to gate the
            // notify push.
            let skip_still_active = if skipped_release.is_empty() {
                false
            } else {
                match is_upgrade(skipped_release, &version) {
                    Ok(true) => {
                        // Live version is strictly newer than the
                        // skipped one — clear and re-notify.
                        if let Err(e) = library::settings::updates::reset_skipped_release(state) {
                            log::warn!("updater_daily: reset_skipped_release: {e}");
                        }
                        false
                    }
                    Ok(false) => true,
                    Err(e) => {
                        // Malformed stored skip version — clear so we
                        // don't permanently mute notifications.
                        log::warn!(
                            "updater_daily: stored skipped_release {skipped_release:?} \
                             not valid semver ({e}); clearing"
                        );
                        if let Err(e) = library::settings::updates::reset_skipped_release(state) {
                            log::warn!("updater_daily: reset_skipped_release: {e}");
                        }
                        false
                    }
                }
            };

            set_update_available(weak, version.clone(), notes_short.clone(), critical);
            persist_success(state, now, Some(version.clone()), etag);

            // Critical releases bypass the skip filter — the user
            // shouldn't be able to permanently mute a release the
            // publisher flagged as not-skippable. They can still
            // dismiss the toast for the current session; it returns
            // at the next daily check.
            if critical || !skip_still_active {
                let _ = event_tx.send(Some(UpdaterEvent::Available {
                    version,
                    notes_short,
                    critical,
                }));
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
mod tests {
    use super::*;

    // These tests cover the pure-logic helpers — anything async or
    // anything that requires the Slint event loop is left for
    // integration testing (the full flow goes through
    // `services::updater::*` which has its own tests).

    #[test]
    fn needs_check_returns_true_when_never_checked() {
        assert!(needs_check(0));
        assert!(needs_check(-1));
    }

    #[test]
    fn needs_check_returns_true_after_24h() {
        let now = Utc::now().timestamp();
        assert!(needs_check(now - ONE_DAY_SECS));
        assert!(needs_check(now - (ONE_DAY_SECS + 1)));
    }

    #[test]
    fn needs_check_returns_false_inside_24h() {
        let now = Utc::now().timestamp();
        // 1 minute ago.
        assert!(!needs_check(now - 60));
        // 23h59m ago.
        assert!(!needs_check(now - (ONE_DAY_SECS - 60)));
    }

    #[test]
    fn needs_check_returns_false_for_future_timestamp() {
        // A `last_check_unix` ahead of `now` (clock skew, NTP step)
        // produces a negative elapsed time. `needs_check` compares
        // against `ONE_DAY_SECS` and so returns false — we skip the
        // check rather than re-firing, which is the safe behavior
        // (a check from "the future" suggests an unreliable clock).
        let future = Utc::now().timestamp() + 3600;
        assert!(!needs_check(future));
    }

    #[test]
    fn backoff_delay_for_zero_failures_is_normal_cadence() {
        assert_eq!(backoff_delay_for(0), NORMAL_CADENCE);
    }

    #[test]
    fn backoff_delay_for_one_failure_is_normal_cadence() {
        // Single hiccup shouldn't extend the cadence — only repeated
        // failures back off.
        assert_eq!(backoff_delay_for(1), NORMAL_CADENCE);
    }

    #[test]
    fn backoff_delay_for_climbs_the_ladder() {
        assert_eq!(backoff_delay_for(2), Duration::from_hours(12));
        assert_eq!(backoff_delay_for(3), Duration::from_hours(24));
        assert_eq!(backoff_delay_for(4), Duration::from_hours(7 * 24));
    }

    #[test]
    fn backoff_delay_for_caps_at_seven_days() {
        // Years of consecutive failures shouldn't blow past the
        // ladder; the saturating add on the counter + ladder index
        // clamp keep us at the 7d ceiling forever.
        assert_eq!(backoff_delay_for(u8::MAX), Duration::from_hours(7 * 24));
        assert_eq!(backoff_delay_for(100), Duration::from_hours(7 * 24));
    }
}
