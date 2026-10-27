//! Has the window painted lately?
//!
//! The strip's Timer keeps ticking for a window the compositor has stopped showing. A tray hide
//! and our minimize drop [`tray_bridge`](crate::ui::shell::tray_bridge)'s visibility shadow, but
//! an OS minimize only sometimes does and Wayland never says. A window nobody shows gets no
//! frames, which catches a Wayland minimize and, for free, a fully covered window.
//!
//! An inference, so it gates only the expensive half and never stops the Timer, which is what
//! lets the tick see frames come back. The count moves only because the tick dirties a property
//! every frame, so it means nothing across a span the tick wasn't running.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, RenderingState};

use melodia_core::error::describe;
use melodia_ui::AppWindow;

/// Never read for its absolute value, only for whether it moved.
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// `true` once the notifier is installed and [`FRAMES`] means something. Latched for the
/// process: a tray hide suspends the graphics context but the renderer keeps the notifier
/// across it, so there is nothing to re-install.
static COUNTING: AtomicBool = AtomicBool::new(false);

/// `true` once [`install`] has run, successfully or not. Gates the call rather than its
/// result: `set_rendering_notifier` swaps the new callback in *before* it decides to
/// return `AlreadySet`, so a second call would silently replace ours.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Called from the first `set-active` rather than at startup: a live notifier costs the renderer
/// an extra flush on every frame of the whole window, which a user who never opens Now Playing
/// shouldn't pay. The deferral loses the window's first `RenderingSetup` (a re-show fires another),
/// so anything needing that phase moves the install back to startup.
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
        Err(e) => log::warn!(
            "visualizer: no frame notifier ({}) — falling back to the shadow",
            describe(&e)
        ),
    }
}

/// Frames drawn so far, or `None` when nothing is counting them — where the caller should
/// assume the window is being painted.
pub(super) fn frames() -> Option<u64> {
    COUNTING.load(Ordering::Relaxed).then(|| FRAMES.load(Ordering::Relaxed))
}
