//! Persisted window geometry restore + snapshot.
//!
//! ## Restore
//!
//! Runs in `main.rs` between `AppWindow::new()` and `app.run()`:
//!
//! - **Size + position** — [`restore`] calls `slint::Window::set_size` /
//!   `set_position`. `set_size` flips `has_explicit_size` in
//!   `i-slint-backend-winit`'s `winitwindowadapter.rs`, and that flag —
//!   re-read at first show — is what stops Slint snapping the window to the
//!   component's content-preferred size; without it the window opens at the
//!   layout minimum. The `WindowAttributes` hook's `with_inner_size` does
//!   *not* set the flag, so it can't be used for size.
//!
//!   Both calls land before the winit window exists, so they're stored into
//!   the pending `WindowAttributes` — i.e. they become the ordinary WM
//!   size/position request. KDE window rules ("Force" / "Apply Initially")
//!   intercept that at the compositor level and win; `has_explicit_size` also
//!   suppresses the post-map preferred-size resize, so we never fight an
//!   "Apply Initially" rule afterwards either.
//! - **Maximized** — no `slint::Window` API exists, so `main.rs`'s
//!   `BackendSelector::with_winit_window_attributes_hook` applies
//!   `WindowAttributes::with_maximized(true)`. The hook runs during
//!   `AppWindow::new()`, so the window is created already maximized — no
//!   un-maximize→maximize flash.
//!
//! ## Save
//!
//! `shutdown::save_state_on_exit` runs after `app.run()` returns, by which
//! point the winit window is destroyed and
//! `WinitWindowAccessor::with_winit_window` returns `None` — so geometry
//! can't be read from winit at shutdown. Instead [`super::winit_filter`]'s
//! `Resized` / `Moved` handlers call [`record`] to keep an in-memory mirror
//! while the window is still alive, and [`snapshot_into`] reads it at exit.
//!
//! Wayland caveat: the compositor security model forbids clients from
//! setting their own position, so `set_position` is a silent no-op on
//! Wayland (KDE Plasma 6 default). Size still works. Use a KDE window
//! rule for positioning on Wayland.

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

/// Lower bound for restored window size, mirroring `min-width` /
/// `min-height` in `ui/app-window.slint`. Guards against a corrupt
/// `settings.json` producing a 0×0 window.
const MIN_RESTORE_WIDTH: f64 = 640.0;
const MIN_RESTORE_HEIGHT: f64 = 420.0;

/// Window geometry persisted in `settings.json`. Built from
/// `read_settings` at startup ([`from_settings`](Self::from_settings))
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

    /// First-launch fallback when `settings.json` couldn't be read —
    /// the same window defaults `SettingsData::default()` carries, so
    /// the window always opens at a sane size.
    pub fn fallback() -> Self {
        Self::from_settings(&SettingsData::default())
    }
}

/// Restore the persisted window size + position, and seed
/// `WindowChrome.is-maximized` from persisted state. Must run after
/// `AppWindow::new()` (the window adapter must exist) and before
/// `app.run()` (before the window is shown). See the module docs for
/// why `set_size` is required rather than the `WindowAttributes` hook.
pub fn restore(app: &AppWindow, geom: PersistedGeometry) {
    // Persisted geometry is f64 (settings.json); Slint's LogicalSize /
    // LogicalPosition use f32. Window pixel coordinates are small
    // integers, well within f32's exact-integer range — same rationale
    // as `boot::ui_setup::apply_sidebar_width`.
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

    // Seed the maximized flag from persisted state, not from the winit
    // window — `with_winit_window` returns `None` before `app.run()`,
    // the window doesn't exist yet. `app-window.slint` keys its
    // transparent-rounded vs opaque-square background mode on
    // `WindowChrome.is-maximized`; seeding it here means a window
    // restored maximized paints square from the first frame instead of
    // briefly flashing the rounded chrome until the first `Resized`.
    app.global::<crate::WindowChrome>()
        .set_is_maximized(geom.maximized);
}

/// Live-mirror payload: the geometry plus whether winit ever reported a
/// real window position.
#[derive(Debug, Clone, Copy)]
struct LiveGeometry {
    geom: PersistedGeometry,
    /// `true` once `outer_position()` has succeeded at least once.
    /// Stays `false` on Wayland — the compositor security model forbids
    /// a client from reading its own position — so [`snapshot_into`]
    /// skips persisting x/y rather than degrading `settings.json` to
    /// `0, 0`.
    position_known: bool,
}

/// Live in-memory mirror of the current window geometry, fed by every
/// winit `Resized` / `Moved` event via [`record`]. `None` until the
/// first event fires — winit emits a synthetic `Resized` on first map,
/// so this is populated before the user can close the window.
static LIVE_GEOMETRY: OnceLock<Mutex<Option<LiveGeometry>>> = OnceLock::new();

fn live() -> &'static Mutex<Option<LiveGeometry>> {
    LIVE_GEOMETRY.get_or_init(|| Mutex::new(None))
}

/// Update the live mirror from the current winit window state. Called
/// by `winit_filter::install`'s `Resized` and `Moved` handlers — both
/// fire while the winit window is still alive, so we capture geometry
/// continuously and don't depend on a post-`app.run()` read (which
/// returns `None` because the winit window is destroyed by the time
/// `save_state_on_exit` runs in `main()`).
///
/// Skips updating size/position while maximized — winit's `inner_size`
/// at that point is the maximized screen size, and persisting it would
/// clobber the user's real restore geometry. The `maximized` flag
/// itself is always recorded.
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

/// Convert a `u32` monitor / window dimension to `i32`, saturating
/// instead of wrapping. Real display dimensions sit far below
/// `i32::MAX`, so the saturating branch is unreachable in practice — it
/// just keeps the conversion lint-clean without an `as` cast.
fn dim_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// One-shot off-screen recovery, run from the first `WindowEvent::Resized`
/// (winit emits a synthetic one on first map, by which point
/// `available_monitors()` is populated). If the restored, non-maximized
/// window rect overlaps no connected monitor — e.g. it was last on a
/// display that has since been unplugged — re-center it on the primary
/// monitor so it stays reachable; the custom titlebar offers no native
/// "move window" affordance for a window that opens off-screen.
///
/// No-op on Wayland: `outer_position()` errors there and the compositor
/// never places a window off-screen anyway. No-op while maximized — the
/// WM remaps maximized windows onto a surviving monitor itself.
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

    let Some(target) = w
        .primary_monitor()
        .or_else(|| w.available_monitors().next())
    else {
        return;
    };
    let mp = target.position();
    let ms = target.size();
    let cx = mp.x + (dim_i32(ms.width) - ww).max(0) / 2;
    let cy = mp.y + (dim_i32(ms.height) - wh).max(0) / 2;
    w.set_outer_position(WinitPhysicalPosition::new(cx, cy));
    log::info!("restored window was off-screen; re-centered to ({cx}, {cy})");
}

/// Snapshot the live geometry mirror into `settings`. Caller already
/// holds a `&mut SettingsData` from `read_settings`. No-op if no
/// `Resized` / `Moved` event ever fired (the mirror is still `None`) —
/// the existing values in `settings` are then left untouched, which is
/// correct: nothing changed this session.
pub fn snapshot_into(settings: &mut SettingsData) {
    let Some(entry) = *live().lock() else {
        return;
    };
    settings.window.is_maximized = entry.geom.maximized;
    if !entry.geom.maximized {
        settings.window_width = entry.geom.width;
        settings.window_height = entry.geom.height;
        // Only persist position when winit actually reported one —
        // never on Wayland. See `LiveGeometry::position_known`.
        if entry.position_known {
            settings.window_x = entry.geom.x;
            settings.window_y = entry.geom.y;
        }
    }
}
