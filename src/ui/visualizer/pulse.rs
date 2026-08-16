//! Has the window painted lately?
//!
//! The strip's Timer fires off the event loop, so it keeps ticking for a window the
//! compositor has stopped showing. A tray hide and our own minimize button both drop
//! [`tray_bridge`](crate::ui::shell::tray_bridge)'s visibility shadow, but an OS-driven
//! minimize only sometimes does and Wayland never tells a client it was minimized at all.
//! Frames cover the rest: a window nobody is showing gets no frame callbacks, which
//! catches a Wayland minimize and, for free, a fully covered window.
//!
//! An *inference* rather than a fact, so it gates only the expensive half and never stops
//! the Timer — which is what lets the tick see frames come back with no event to wait for.
//! The count moves only because the tick dirties a property every frame, so it means
//! nothing across a span where the tick wasn't running; see
//! [`FRAME_STALL_TICKS`](super::FRAME_STALL_TICKS).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, RenderingState};

use crate::AppWindow;

/// Frames drawn since startup. Monotonic, and never read for its absolute value —
/// the tick only asks whether it moved since last time.
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// `true` once the notifier is installed and [`FRAMES`] means something. Latched for the
/// process: a tray hide suspends the graphics context but the renderer keeps the notifier
/// across it, so there is nothing to re-install.
static COUNTING: AtomicBool = AtomicBool::new(false);

/// `true` once [`install`] has run, successfully or not. Gates the call rather than its
/// result: `set_rendering_notifier` swaps the new callback in *before* it decides to
/// return `AlreadySet`, so a second call would silently replace ours.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Start counting. Called from `set-active` the first time a strip mounts rather than at
/// startup: a live notifier costs the renderer an extra flush on every drawn frame of the
/// *whole window*, and a user who never opens Now Playing shouldn't pay for a signal
/// nothing is reading.
///
/// Deferring costs the window's *first* `RenderingState::RenderingSetup`, long gone by the
/// time this lands — a re-show fires another, so that phase is unreliable here rather than
/// absent. Anything needing setup moves the install back to startup.
pub(super) fn install(ui: &AppWindow) {
    if INSTALLED.swap(true, Ordering::Relaxed) {
        return;
    }
    match ui.window().set_rendering_notifier(|state, _| {
        if matches!(state, RenderingState::BeforeRendering) {
            FRAMES.fetch_add(1, Ordering::Relaxed);
        }
    }) {
        Ok(()) => COUNTING.store(true, Ordering::Relaxed),
        // Only the software renderer refuses, and this build is FemtoVG-only.
        Err(e) => log::warn!("visualizer: no frame notifier ({e}) — falling back to the shadow"),
    }
}

/// Frames drawn so far, or `None` when nothing is counting them — where the caller should
/// assume the window is being painted.
pub(super) fn frames() -> Option<u64> {
    COUNTING.load(Ordering::Relaxed).then(|| FRAMES.load(Ordering::Relaxed))
}
