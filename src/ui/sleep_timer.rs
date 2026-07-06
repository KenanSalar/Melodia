//! Session-only sleep timer wired to the Now-Playing overflow menu.
//!
//! Modes, selected from `Player.set-sleep-timer(minutes)` (presets) and
//! `Player.set-sleep-timer-seconds(seconds)` (the custom ±stepper):
//! - **Duration** (`minutes > 0`, or any stepper seconds): a cancellable tokio
//!   countdown that cleanly pauses playback after the chosen duration, ticking a
//!   live remaining string into `Player.sleep-timer-remaining` each second. The
//!   countdown is **playback-linked** — it only burns down while `Playing`
//!   (checked via the lock-free status mirror), so pausing the music holds the
//!   timer and it never expires on a paused/stopped player.
//! - **End of current track** (`minutes < 0`): arms the backend
//!   `PlayerState::pause_after_current_track` flag (via
//!   [`library::playback::player_set_pause_at_track_end`]); the playback monitor
//!   pauses at the next track boundary. Its "armed" state rides on
//!   `Player.vm.sleep_at_track_end`, so the UI auto-clears when it fires.
//! - **Off** (`minutes == 0`): cancels any duration timer and disarms the flag.
//!
//! The armed *duration* (seconds) is mirrored to the UI via
//! `Player.sleep-timer-total-seconds` so the flyout can highlight a matching
//! preset (or none, for a custom value).
//!
//! This lives in `ui/` (not `tasks/`) deliberately: the countdown task must
//! write a Slint property, and `tasks/` may not import `ui::*`. The cancel token
//! is held in an `Rc<RefCell<..>>` captured by the callback — callbacks run
//! UI-thread-only, so no `AppState` widening is needed; only the token *clone*
//! moved into the spawned task must be `Send` (it is).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use slint::ComponentHandle;
use tokio_util::sync::CancellationToken;

use crate::library;
use crate::player::types::PlaybackStatus;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use crate::{AppWindow, Player};

/// Minimum custom sleep-timer duration (seconds) — 0:30. Mirrors the stepper's
/// floor in `sleep-timer-flyout.slint`.
const MIN_SLEEP_SECONDS: i32 = 30;
/// Maximum custom sleep-timer duration (seconds) — 2 hours. Bounded so the
/// display and hold-to-accelerate stay tidy; mirrors the stepper's ceiling.
const MAX_SLEEP_SECONDS: i32 = 2 * 60 * 60;

/// Clamp a stepper-provided seconds value into `[MIN, MAX]`. Defensive — the
/// stepper already clamps in Slint, but the callback boundary shouldn't trust it.
fn clamp_sleep_seconds(secs: i32) -> i32 {
    secs.clamp(MIN_SLEEP_SECONDS, MAX_SLEEP_SECONDS)
}

/// Format whole seconds as `M:SS` (e.g. 90 → `"1:30"`), or `H:MM:SS` once an
/// hour or more (e.g. 5400 → `"1:30:00"`) so long timers read naturally.
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

/// Reset the session-only display props to "no duration timer" (total 0, empty
/// remaining). Used by the Off and End-of-track paths, and by the countdown
/// task on fire.
fn reset_display(weak: &slint::Weak<AppWindow>) {
    if let Some(ui) = weak.upgrade() {
        let p = ui.global::<Player>();
        p.set_sleep_timer_total_seconds(0);
        p.set_sleep_timer_remaining(slint::SharedString::new());
    }
}

/// Wire `Player.set-sleep-timer` (preset picks) and `Player.set-sleep-timer-seconds`
/// (the custom ±stepper). Call once after constructing `AppWindow` (alongside
/// `install_equalizer` / `install_replaygain`).
pub fn install_sleep_timer(ui: &AppWindow, state: &AppState) {
    let player = ui.global::<Player>();
    // UI-thread-only cancel-and-replace store for the active duration timer,
    // shared by both callbacks below (they all run on the UI thread).
    let store: Rc<RefCell<Option<CancellationToken>>> = Rc::new(RefCell::new(None));

    // Preset picks: 0 = off, > 0 = duration (minutes), -1 = "End of track".
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

/// Apply a *preset* sleep-timer selection. `minutes`: 0 = off, `> 0` = duration
/// (converted to seconds), `-1` = "End of track".
fn arm_sleep_timer(
    state: &AppState,
    weak: &slint::Weak<AppWindow>,
    store: &Rc<RefCell<Option<CancellationToken>>>,
    minutes: i32,
) {
    let ctx = state.playback_ctx();

    match minutes.cmp(&0) {
        std::cmp::Ordering::Equal => {
            // Off — cancel any timer, disarm end-of-track mode, clear display.
            cancel_timer(store);
            let _ = library::playback::player_set_pause_at_track_end(&ctx, false);
            reset_display(weak);
        }
        std::cmp::Ordering::Less => {
            // End of current track — cancel any timer, arm the backend flag; no
            // countdown. The row shows "Track end" from `vm.sleep_at_track_end`.
            cancel_timer(store);
            let _ = library::playback::player_set_pause_at_track_end(&ctx, true);
            reset_display(weak);
        }
        std::cmp::Ordering::Greater => {
            arm_duration_seconds(state, weak, store, minutes.saturating_mul(60));
        }
    }
}

/// Arm a duration countdown for `total_secs` seconds. Shared by the minute
/// presets and the custom stepper. Cancels any running timer, clears
/// end-of-track mode, seeds the display, and spawns the countdown.
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

/// Cancel and clear any running duration timer (any new selection supersedes
/// the previous one).
fn cancel_timer(store: &Rc<RefCell<Option<CancellationToken>>>) {
    if let Some(tok) = store.borrow_mut().take() {
        tok.cancel();
    }
}

/// Spawn the per-second countdown. Mirrors `tasks/heap_trim.rs`'s
/// `spawn_cancellable` + `select!` shape, extended to three arms (global
/// shutdown, this timer's own cancel token, and the 1 Hz interval). On reaching
/// zero it cleanly pauses via `player_pause` and clears the display.
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
        // The first `interval` tick fires immediately — consume it so each
        // loop tick is a true 1 s elapse.
        ticker.tick().await;
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = token.cancelled() => return,
                _ = ticker.tick() => {
                    // Playback-linked: only burn down while actually Playing, so
                    // pausing the music holds the countdown (and it never expires
                    // on a paused/stopped player). Read the lock-free status
                    // mirror — no PlayerState lock needed on the tick.
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
