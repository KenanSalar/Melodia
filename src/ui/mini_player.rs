//! Miniplayer module — wires the `MiniPlayer` Slint global's
//! `active-changed` and `exit-pressed` callbacks to OS-level window-size
//! mgmt and the Up Next subscriber's visibility gate.
//!
//! ## Resize-only trigger
//!
//! Tauri parity: the miniplayer is engaged purely by the OS window being
//! shrunk past the threshold derived inside `ui/app-window.slint`
//! (`mini-active: self.width < 550px || self.height < 250px`). There is
//! no entry button anywhere in the full UI. The only mini-specific
//! affordance is the `open_in_full` exit button inside the mini view
//! itself, which restores the window to its pre-mini geometry — and from
//! that the size derivation naturally flips `mini-active = false`.
//!
//! ## Pre-mini geometry snapshot
//!
//! Each time `MiniPlayer.active` flips true, we capture the current
//! `PersistedGeometry` from the live mirror in
//! [`crate::ui::window_chrome::geometry`]. On `exit-pressed` we ask winit
//! to resize the window back to that snapshot. Repeated enter/exit
//! cycles via resize re-capture on each enter, so the snapshot always
//! reflects the last "full UI" size before the most recent shrink. If
//! the user shrank before any winit `Resized` event fired (snapshot is
//! `None`), the fallback `800x600` matches `SettingsData::default()`.
//!
//! ## Why not a module under `window_chrome`
//!
//! `window_chrome` owns OS-frame concerns (the custom titlebar, drag
//! region, restart flow, minimise/maximise). The miniplayer is a
//! responsive *layout* concern that happens to call winit; keeping it
//! sibling-level makes the dataflow greppable and avoids mixing
//! orthogonal axes.

use std::rc::Rc;
use std::sync::LazyLock;

use parking_lot::Mutex;
use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::dpi::LogicalSize;

use crate::error::AppError;
use crate::ui::now_playing::NowPlayingState;
use crate::ui::window_chrome::geometry::{self, PersistedGeometry};
use crate::{AppWindow, MiniPlayer};

/// Pre-mini OS window geometry — captured each time `MiniPlayer.active`
/// flips true, restored when the exit button is pressed. `None` until the
/// first transition into mini. Outlives the install closure (single
/// source of truth across re-entries), so a static is the right shape.
static PRE_MINI_GEOM: LazyLock<Mutex<Option<PersistedGeometry>>> =
    LazyLock::new(|| Mutex::new(None));

/// Fallback restore geometry when no pre-mini snapshot exists (the user
/// shrank the window before any `Resized` event fired). Matches the
/// `SettingsData::default()` window size, so the experience is the same
/// as a fresh-install first launch.
const FALLBACK_RESTORE_WIDTH: f64 = 800.0;
const FALLBACK_RESTORE_HEIGHT: f64 = 600.0;

/// Wire `MiniPlayer.active-changed` and `MiniPlayer.exit-pressed` to
/// pre-mini geometry snapshot/restore and the Up Next subscriber's
/// `mini_visible` gate. Runs on the Slint event-loop thread, between
/// `AppWindow::new()` and `app.run()` — same install window as the
/// `now_playing` / `window_chrome` modules.
pub fn install(app: &AppWindow, np_state: &Rc<NowPlayingState>) -> Result<(), AppError> {
    let mini = app.global::<MiniPlayer>();

    // active-changed: enter/leave mini state. Capture pre-mini geometry
    // on enter, flip the Up Next subscriber gate either way, and re-seed
    // the Up Next list on enter so the square variant doesn't render an
    // empty list when the queue hasn't changed since the subscriber
    // started stashing. Mirrors `wire_now_playing_open`'s on-open seed.
    {
        let np_state = np_state.clone();
        mini.on_active_changed(move |is_active| {
            np_state.mini_visible.set(is_active);
            if is_active {
                if let Some(geom) = geometry::current() {
                    *PRE_MINI_GEOM.lock() = Some(geom);
                }
                np_state.kick_up_next();
            }
        });
    }

    // exit-pressed: restore the snapshot geometry (or fallback) via
    // winit. The resulting `Resized` event naturally flips
    // `mini-active = false` through the size derivation — no imperative
    // property write needed.
    //
    // Wayland note: `set_position` is a no-op there, so we restore size
    // only and let the compositor keep the window where it is. For the
    // common single-monitor case this matches user intent (the window
    // grows from its current position rather than jumping).
    {
        let weak = app.as_weak();
        mini.on_exit_pressed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let snapshot = (*PRE_MINI_GEOM.lock()).unwrap_or(PersistedGeometry {
                width: FALLBACK_RESTORE_WIDTH,
                height: FALLBACK_RESTORE_HEIGHT,
                x: 0.0,
                y: 0.0,
                maximized: false,
            });
            let _ = ui.window().with_winit_window(|w| {
                // The returned `Some(size)` would mean winit rejected the
                // request synchronously and resolved to a clamped size —
                // we'd still see the real change land via the next
                // `Resized` event, which is the path that flips
                // `mini-active`. So the return value is intentionally
                // discarded.
                let _ = w.request_inner_size(LogicalSize::new(snapshot.width, snapshot.height));
            });
        });
    }

    Ok(())
}
