//! Session-only sleep timer wired to the Now-Playing overflow menu.
//!
//! Three modes, picked from `Player.set-sleep-timer(minutes)` for the presets and
//! `set-sleep-timer-seconds` for the custom stepper:
//! - **Duration** — a cancellable tokio countdown that pauses playback after the chosen
//!   time, ticking a remaining string into `Player.sleep-timer-remaining` each second.
//!   It is **playback-linked**, burning down only while `Playing` (read off the
//!   lock-free status mirror), so pausing the music holds the timer and it never expires
//!   on a paused player.
//! - **End of current track** (`minutes < 0`) — arms
//!   `PlayerState::pause_after_current_track` and lets the playback monitor pause at the
//!   next boundary. Its armed state rides on `Player.vm.sleep_at_track_end`, so the UI
//!   auto-clears when it fires.
//! - **Off** — cancels any duration timer and disarms the flag.
//!
//! The armed duration is mirrored through `Player.sleep-timer-total-seconds`, so the
//! flyout can highlight a matching preset or none.
//!
//! In `ui/` rather than `tasks/` because the countdown writes a Slint property and
//! `tasks/` may not import `ui::*`. The cancel token sits in an `Rc<RefCell<…>>` the
//! callback captures — callbacks are UI-thread-only, so only the clone moved into the
//! spawned task has to be `Send`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use slint::ComponentHandle;
use tokio_util::sync::CancellationToken;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_app::tasks::TaskSpawner;
use melodia_engine::player::engine::types::PlaybackStatus;
use melodia_ui::{AppWindow, Player};

/// Bounds on a custom duration, mirroring the stepper's own in
/// `sleep-timer-flyout.slint` — the ceiling keeps the display and the
/// hold-to-accelerate tidy.
const MIN_SLEEP_SECONDS: i32 = 30;
const MAX_SLEEP_SECONDS: i32 = 2 * 60 * 60;

/// Clamp a stepper-provided value. Defensive: the stepper already clamps in Slint, but
/// the callback boundary shouldn't trust it.
fn clamp_sleep_seconds(secs: i32) -> i32 {
    secs.clamp(MIN_SLEEP_SECONDS, MAX_SLEEP_SECONDS)
}

/// Whole seconds as `M:SS`, or `H:MM:SS` once an hour or more, so a long timer reads
/// naturally.
pub fn format_remaining(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Reset the display properties to "no duration timer" — the Off and End-of-track
/// paths, and the countdown task on fire.
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
    // Cancel-and-replace store for the active duration timer, shared by both callbacks
    // — all of it UI-thread-only.
    let store: Rc<RefCell<Option<CancellationToken>>> = Rc::new(RefCell::new(None));

    // Presets: 0 off, > 0 duration in minutes, -1 end of track.
    {
        let state = state.clone();
        let weak = ui.as_weak();
        let store = store.clone();
        player.on_set_sleep_timer(move |minutes| {
            arm_sleep_timer(&state, &weak, &store, minutes);
        });
    }

    // Custom duration from the ±stepper's Start pill (seconds). Clamp + arm.
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
    store: &Rc<RefCell<Option<CancellationToken>>>,
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

/// Arm a duration countdown, shared by the minute presets and the custom stepper:
/// cancel any running timer, clear end-of-track mode, seed the display, spawn.
fn arm_duration_seconds(
    state: &AppState,
    weak: &slint::Weak<AppWindow>,
    store: &Rc<RefCell<Option<CancellationToken>>>,
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

/// Cancel and clear any running duration timer — a new selection supersedes the last.
fn cancel_timer(store: &Rc<RefCell<Option<CancellationToken>>>) {
    if let Some(tok) = store.borrow_mut().take() {
        tok.cancel();
    }
}

/// Spawn the per-second countdown: the usual `spawn_cancellable` + `select!` shape over
/// three arms — global shutdown, this timer's own token, and the 1 Hz interval. Reaching
/// zero pauses cleanly and clears the display.
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
                    // Playback-linked, so pausing the music holds the countdown. Off
                    // the lock-free status mirror: no `PlayerState` lock on the tick.
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
        // Fire: clean pause, then clear the display — unless a newer selection
        // superseded this timer between its final decrement and now. The
        // re-arm path cancels this token and seeds the new display
        // synchronously on the UI thread, so re-checking the token inside the
        // UI closure fully serializes against the re-arm's writes: a stale
        // clear either runs before the re-arm (and is overwritten by the
        // seed) or sees the cancelled token and skips.
        if token.is_cancelled() {
            return;
        }
        if let Err(e) = library::playback::player_pause(&ctx) {
            log::warn!("sleep timer pause: {e}");
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
