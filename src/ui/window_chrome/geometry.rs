//! Persisted window geometry restore + snapshot.
//!
//! **Restore** runs in `main.rs` between `AppWindow::new()` and `app.run()`. Size and
//! position go through `slint::Window::set_size` / `set_position` rather than the
//! `WindowAttributes` hook, because `set_size` flips `has_explicit_size` in the winit
//! backend and that flag, re-read at first show, is what stops Slint snapping the window
//! to its content-preferred size; `with_inner_size` does not set it. Both calls land
//! before the winit window exists, so they become the ordinary WM request and a KDE window
//! rule still wins. **Maximized** has no `slint::Window` API at all, so `main.rs`'s
//! window-attributes hook applies `with_maximized(true)` during `AppWindow::new()` — no
//! un-maximize→maximize flash.
//!
//! **Save** can't read winit: `shutdown::save_state_on_exit` runs after `app.run()`
//! returns, by which point the window is destroyed and `with_winit_window` answers `None`.
//! [`super::winit_filter`]'s `Resized` / `Moved` handlers call [`record`] to keep an
//! in-memory mirror while the window is alive, and [`snapshot_into`] reads it at exit.
//!
//! Wayland forbids a client setting its own position, so `set_position` is a silent no-op
//! there. Size still works.

use std::sync::{Once, OnceLock};

use parking_lot::Mutex;
use slint::winit_030::winit::dpi::{
    LogicalPosition as WinitLogicalPosition, LogicalSize as WinitLogicalSize,
    PhysicalPosition as WinitPhysicalPosition,
};
use slint::winit_030::winit::window::Window as WinitWindow;
use slint::{ComponentHandle, LogicalPosition, LogicalSize};

use crate::AppWindow;
use crate::services::settings::SettingsData;

/// Lower bound for a restored window size, mirroring `app-window.slint`'s own — a guard
/// against a corrupt `settings.json` producing a 0×0 window.
const MIN_RESTORE_WIDTH: f64 = 640.0;
const MIN_RESTORE_HEIGHT: f64 = 420.0;

/// Window geometry persisted in `settings.json`. Built from `read_settings` at startup
/// and from winit events into the live mirror ([`record`]).
#[derive(Debug, Clone, Copy)]
pub struct PersistedGeometry {
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
    pub maximized: bool,
}

impl PersistedGeometry {
    pub fn from_settings(s: &SettingsData) -> Self {
        Self {
            width: s.window_width,
            height: s.window_height,
            x: s.window_x,
            y: s.window_y,
            maximized: s.window.is_maximized,
        }
    }

    /// First-launch fallback when `settings.json` couldn't be read — the same defaults
    /// `SettingsData::default()` carries, so the window always opens at a sane size.
    pub fn fallback() -> Self {
        Self::from_settings(&SettingsData::default())
    }
}

/// Restore the persisted window size and position, and seed `WindowChrome.is-maximized`.
/// Must run after `AppWindow::new()`, the window adapter having to exist, and before
/// `app.run()`.
pub fn restore(app: &AppWindow, geom: PersistedGeometry) {
    // Persisted geometry is `f64`, Slint's logical types `f32` — window pixel coordinates
    // are small integers, well inside `f32`'s exact-integer range.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "window pixel coordinates are small; f32 precision is sufficient"
    )]
    let (w, h, x, y) = (
        geom.width.max(MIN_RESTORE_WIDTH) as f32,
        geom.height.max(MIN_RESTORE_HEIGHT) as f32,
        geom.x as f32,
        geom.y as f32,
    );
    let window = app.window();
    window.set_size(LogicalSize::new(w, h));
    window.set_position(LogicalPosition::new(x, y));

    // From persisted state, not the winit window, which doesn't exist yet.
    // `app-window.slint` keys its transparent-rounded versus opaque-square background on
    // this, so a window restored maximized paints square from the first frame.
    app.global::<crate::WindowChrome>().set_is_maximized(geom.maximized);
}

/// Live-mirror payload: the geometry plus whether winit ever reported a real position.
#[derive(Debug, Clone, Copy)]
struct LiveGeometry {
    geom: PersistedGeometry,
    /// `true` once `outer_position()` has succeeded at least once. Stays `false` on
    /// Wayland, where a client may not read its own position, so [`snapshot_into`] skips
    /// persisting x/y rather than degrading `settings.json` to `0, 0`.
    position_known: bool,
}

/// Live in-memory mirror, fed by every winit `Resized` / `Moved` through [`record`].
/// `None` until the first fires — winit emits a synthetic `Resized` on first map, so it is
/// populated before the user can close the window.
static LIVE_GEOMETRY: OnceLock<Mutex<Option<LiveGeometry>>> = OnceLock::new();

fn live() -> &'static Mutex<Option<LiveGeometry>> {
    LIVE_GEOMETRY.get_or_init(|| Mutex::new(None))
}

/// Update the live mirror from the current winit window state, called by the `Resized` and
/// `Moved` handlers while the winit window is still alive.
///
/// Skips size and position while maximized: winit's `inner_size` there is the maximized
/// screen size, and persisting it would clobber the user's real restore geometry. The
/// `maximized` flag itself is always recorded.
pub fn record(w: &WinitWindow) {
    let scale = w.scale_factor();
    let maximized = w.is_maximized();
    let inner: WinitLogicalSize<f64> = w.inner_size().to_logical(scale);
    let outer: Option<WinitLogicalPosition<f64>> =
        w.outer_position().ok().map(|p| p.to_logical(scale));

    let mut guard = live().lock();
    let entry = guard.get_or_insert(LiveGeometry {
        geom: PersistedGeometry {
            width: inner.width,
            height: inner.height,
            x: outer.map_or(0.0, |p| p.x),
            y: outer.map_or(0.0, |p| p.y),
            maximized,
        },
        position_known: false,
    });
    entry.geom.maximized = maximized;
    if !maximized {
        entry.geom.width = inner.width;
        entry.geom.height = inner.height;
        if let Some(p) = outer {
            entry.geom.x = p.x;
            entry.geom.y = p.y;
            entry.position_known = true;
        }
    }
}

/// A `u32` monitor or window dimension as `i32`, saturating rather than wrapping — real
/// display dimensions sit far below `i32::MAX`, so that branch is unreachable.
fn dim_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// One-shot off-screen recovery, run from the first `WindowEvent::Resized` — winit emits a
/// synthetic one on first map, by which point `available_monitors()` is populated. A
/// restored, non-maximized rect overlapping no connected monitor is re-centred so it stays
/// reachable, the custom titlebar offering no native "move window" affordance.
///
/// No-op on Wayland, where `outer_position()` errors and the compositor never places a
/// window off-screen, and while maximized, the WM remapping those itself.
pub fn ensure_on_screen(w: &WinitWindow) {
    static DONE: Once = Once::new();
    let mut first = false;
    DONE.call_once(|| first = true);
    if !first {
        return;
    }

    if w.is_maximized() {
        return;
    }
    let Ok(pos) = w.outer_position() else {
        return; // Wayland — position is compositor-managed.
    };
    let size = w.outer_size();
    let (wx, wy) = (pos.x, pos.y);
    let (ww, wh) = (dim_i32(size.width), dim_i32(size.height));

    let overlaps = w.available_monitors().any(|m| {
        let mp = m.position();
        let ms = m.size();
        wx < mp.x + dim_i32(ms.width)
            && wx + ww > mp.x
            && wy < mp.y + dim_i32(ms.height)
            && wy + wh > mp.y
    });
    if overlaps {
        return;
    }

    let Some(target) = w.primary_monitor().or_else(|| w.available_monitors().next()) else {
        return;
    };
    let mp = target.position();
    let ms = target.size();
    let cx = mp.x + (dim_i32(ms.width) - ww).max(0) / 2;
    let cy = mp.y + (dim_i32(ms.height) - wh).max(0) / 2;
    w.set_outer_position(WinitPhysicalPosition::new(cx, cy));
    log::info!("restored window was off-screen; re-centered to ({cx}, {cy})");
}

/// Snapshot the live geometry mirror into `settings`. A no-op if no `Resized` / `Moved`
/// ever fired, which leaves the existing values untouched — correct, nothing having
/// changed this session.
pub fn snapshot_into(settings: &mut SettingsData) {
    let Some(entry) = *live().lock() else {
        return;
    };
    settings.window.is_maximized = entry.geom.maximized;
    if !entry.geom.maximized {
        settings.window_width = entry.geom.width;
        settings.window_height = entry.geom.height;
        // Only when winit actually reported one — never on Wayland.
        if entry.position_known {
            settings.window_x = entry.geom.x;
            settings.window_y = entry.geom.y;
        }
    }
}
