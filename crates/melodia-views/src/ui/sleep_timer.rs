//! The session-only sleep timer behind the Now Playing overflow menu, in three modes off
//! `Player.set-sleep-timer(minutes)` (the presets) and `set-sleep-timer-seconds` (the stepper):
//! - **Duration**, a cancellable countdown that pauses playback when it runs out. It burns down
//!   only while `Playing`, read off the lock-free status mirror, so a paused player holds it.
//! - **End of current track** (`minutes < 0`), arming `PlayerState::pause_after_current_track`
//!   for the playback monitor; `Player.vm.sleep_at_track_end` clears the UI when it fires.
//! - **Off**, cancelling the countdown and disarming the flag.
//!
//! `Player.sleep-timer-total-seconds` mirrors the armed duration for the flyout's preset
//! highlight. In `ui/` rather than `tasks/` because the countdown writes a Slint property.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use slint::ComponentHandle;
use tokio_util::sync::CancellationToken;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_app::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_engine::player::engine::types::PlaybackStatus;
use melodia_ui::{AppWindow, Player};

/// Bounds on a custom duration, mirroring the stepper's own in `sleep-timer-flyout.slint`.
const MIN_SLEEP_SECONDS: i32 = 30;
const MAX_SLEEP_SECONDS: i32 = 2 * 60 * 60;

/// The stepper clamps in Slint too, but the callback boundary doesn't trust it.
fn clamp_sleep_seconds(secs: i32) -> i32 {
    secs.clamp(MIN_SLEEP_SECONDS, MAX_SLEEP_SECONDS)
}

/// Whole seconds as `M:SS`, or `H:MM:SS` once an hour or more.
pub fn format_remaining(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Reset the display to "no countdown", for Off, End of track and a countdown that fired.
fn reset_display(weak: &slint::Weak<AppWindow>) {
    if let Some(ui) = weak.upgrade() {
        let p = ui.global::<Player>();
        p.set_sleep_timer_total_seconds(0);
        p.set_sleep_timer_remaining(slint::SharedString::new());
    }
}

/// Wire both callbacks. Call once after constructing `AppWindow`, beside
/// `install_equalizer` and `install_replaygain`.
pub fn install_sleep_timer(ui: &AppWindow, state: &AppState) {
    let player = ui.global::<Player>();
    // Cancel-and-replace store for the running countdown; UI thread only.
    let store: Rc<RefCell<Option<CancellationToken>>> = Rc::new(RefCell::new(None));

    {
        let state = state.clone();
        let weak = ui.as_weak();
        let store = store.clone();
        player.on_set_sleep_timer(move |minutes| {
            arm_sleep_timer(&state, &weak, &store, minutes);
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        let store = store.clone();
        player.on_set_sleep_timer_seconds(move |seconds| {
            arm_duration_seconds(&state, &weak, &store, clamp_sleep_seconds(seconds));
        });
    }
}

/// Apply a *preset* selection: 0 off, `> 0` a duration in minutes, `-1` end of track.
fn arm_sleep_timer(
    state: &AppState,
    weak: &slint::Weak<AppWindow>,
    store: &RefCell<Option<CancellationToken>>,
    minutes: i32,
) {
    let ctx = state.playback_ctx();

    match minutes.cmp(&0) {
        std::cmp::Ordering::Equal => {
            cancel_timer(store);
            let _ = library::playback::player_set_pause_at_track_end(&ctx, false);
            reset_display(weak);
        }
        std::cmp::Ordering::Less => {
            // No countdown: the row shows "Track end" off `vm.sleep_at_track_end`.
            cancel_timer(store);
            let _ = library::playback::player_set_pause_at_track_end(&ctx, true);
            reset_display(weak);
        }
        std::cmp::Ordering::Greater => {
            arm_duration_seconds(state, weak, store, minutes.saturating_mul(60));
        }
    }
}

/// Arm a countdown of `total_secs`, for the presets and the stepper alike.
fn arm_duration_seconds(
    state: &AppState,
    weak: &slint::Weak<AppWindow>,
    store: &RefCell<Option<CancellationToken>>,
    total_secs: i32,
) {
    cancel_timer(store);
    let ctx = state.playback_ctx();
    let _ = library::playback::player_set_pause_at_track_end(&ctx, false);

    let total = u64::try_from(total_secs).unwrap_or(0);
    if let Some(ui) = weak.upgrade() {
        let p = ui.global::<Player>();
        p.set_sleep_timer_total_seconds(total_secs);
        p.set_sleep_timer_remaining(format_remaining(total).into());
    }
    let token = CancellationToken::new();
    *store.borrow_mut() = Some(token.clone());
    spawn_countdown(state, weak.clone(), token, total);
}

fn cancel_timer(store: &RefCell<Option<CancellationToken>>) {
    if let Some(tok) = store.borrow_mut().take() {
        tok.cancel();
    }
}

/// Spawn the 1 Hz countdown, which pauses playback and clears the display on reaching zero.
fn spawn_countdown(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    token: CancellationToken,
    total_secs: u64,
) {
    let spawner = TaskSpawner::from_state(state);
    let ctx = state.playback_ctx();
    spawner.spawn_cancellable(move |shutdown| async move {
        let mut remaining = total_secs;
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        // The first `interval` tick fires immediately; consume it so each loop tick is
        // a true one-second elapse.
        ticker.tick().await;
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = token.cancelled() => return,
                _ = ticker.tick() => {
                    // The lock-free mirror, so the tick takes no `PlayerState` lock.
                    let playing = ctx.player_state.status_atomic.load(Ordering::Relaxed)
                        == PlaybackStatus::Playing as u8;
                    if !playing {
                        continue;
                    }
                    remaining = remaining.saturating_sub(1);
                    let text = format_remaining(remaining);
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<Player>().set_sleep_timer_remaining(text.into());
                    });
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }
        // A newer selection may have superseded this timer since its last decrement. The
        // re-arm cancels the token and seeds its display synchronously on the UI thread, so
        // re-checking the token in the UI closure serializes against it: a stale clear either
        // runs before the seed or sees the cancel and skips.
        if token.is_cancelled() {
            return;
        }
        if let Err(e) = library::playback::player_pause(&ctx) {
            log::warn!("sleep timer pause: {}", describe(&e));
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if token.is_cancelled() {
                return;
            }
            let p = ui.global::<Player>();
            p.set_sleep_timer_total_seconds(0);
            p.set_sleep_timer_remaining(slint::SharedString::new());
        });
    });
}

#[cfg(test)]
#[path = "tests/sleep_timer_tests.rs"]
mod tests;
