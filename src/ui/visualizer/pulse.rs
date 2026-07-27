//! Has the window painted lately?
//!
//! The strip's Timer fires off the event loop, so it keeps ticking for a window
//! the compositor has stopped showing. Two of those cases announce themselves —
//! a tray hide and our own minimize button both drop
//! [`tray_bridge`](crate::ui::tray_bridge)'s visibility shadow — but an OS-driven
//! minimize only sometimes does, and on Wayland a client is never told it was
//! minimized at all (`xdg_toplevel` has `set_minimized` and no inverse).
//!
//! Frames are the signal that covers the rest. A window nobody is showing gets no
//! frame callbacks, so it isn't drawn; this counts the draws and lets the tick
//! notice they stopped. That catches a Wayland minimize and, for free, a window
//! fully covered by another one.
//!
//! It is an *inference*, not a fact, which is why it gates only the expensive
//! half — the transforms and the audio-thread tap — and never stops the Timer.
//! Leaving the Timer running is what lets the tick see frames come back on its
//! own, with no event to wait for.
//!
//! Reading it correctly is the caller's half of the bargain: the count only
//! moves because the tick dirties a property every frame, so it means nothing
//! across a span where the tick wasn't running. See
//! [`FRAME_STALL_TICKS`](super::FRAME_STALL_TICKS).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, RenderingState};

use crate::AppWindow;

/// Frames drawn since startup. Monotonic, and never read for its absolute value —
/// the tick only asks whether it moved since last time.
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// `true` once the notifier is installed and [`FRAMES`] means something.
static COUNTING: AtomicBool = AtomicBool::new(false);

/// Start counting. Called once, from `install_visualizer`.
pub(super) fn install(ui: &AppWindow) {
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

/// Frames drawn so far, or `None` when nothing is counting them — in which case
/// there is nothing to infer from and the caller should assume the window is
/// being painted.
pub(super) fn frames() -> Option<u64> {
    COUNTING
        .load(Ordering::Relaxed)
        .then(|| FRAMES.load(Ordering::Relaxed))
}
