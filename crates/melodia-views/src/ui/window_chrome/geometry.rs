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
//!
//! **The miniplayer is a window size, so the full player's geometry has to be held across it.**
//! Entering it [`hold_full_player`]s the last placement that sat still, and its restore button
//! hands that back through [`restore_full_player`]. A close while it is up persists the miniplayer
//! as the window and the hold beside it, so the next launch reopens the miniplayer and its restore
//! button still knows the player the user shrank away from.
//!
//! **The frame the miniplayer drops is measured as the step it costs the client, not asked of the
//! window.** [`frame_allowance`] carries the argument; the exit edge and the restore both want that
//! step and nothing else, so there is one number rather than a query per platform.

use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
#[cfg(target_os = "windows")]
use slint::winit_030::winit::dpi::PhysicalInsets as WinitPhysicalInsets;
use slint::winit_030::winit::dpi::{
    LogicalInsets as WinitLogicalInsets, LogicalPosition as WinitLogicalPosition,
    LogicalSize as WinitLogicalSize, PhysicalPosition as WinitPhysicalPosition,
    PhysicalSize as WinitPhysicalSize,
};
use slint::winit_030::winit::window::Window as WinitWindow;
use slint::{ComponentHandle, LogicalPosition, LogicalSize};

use melodia_app::services::settings::{FullPlayerGeometry, SettingsData, WindowPosition};
use melodia_ui::{AppWindow, MiniPlayer};

/// Lower bound for the full player the restore button lands on, past the miniplayer's exit edge so
/// a corrupt hold can't leave the button resizing the miniplayer into itself.
const MIN_FULL_PLAYER: LogicalSize = LogicalSize::new(640.0, 420.0);

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

/// Restore the persisted window size and position, the full player held behind a miniplayer it
/// closed as, and seed `WindowChrome.is-maximized`. Must run after `AppWindow::new()`, the window
/// adapter having to exist, and before `app.run()`.
pub fn restore(app: &AppWindow, geom: PersistedGeometry, full_player: Option<FullPlayerGeometry>) {
    let mini = app.global::<MiniPlayer>();
    let floor = LogicalSize::new(mini.get_window_min_width(), mini.get_window_min_height());
    let (size, position) = logical_placement(geom, floor);
    let window = app.window();
    window.set_size(size);
    window.set_position(position);

    // From persisted state, not the winit window, which doesn't exist yet.
    // `app-window.slint` keys its transparent-rounded versus opaque-square background on
    // this, so a window restored maximized paints square from the first frame.
    app.global::<melodia_ui::WindowChrome>().set_is_maximized(geom.maximized);

    // After `set_size`, which is what hands the switch the size it decides on.
    app.invoke_adopt_restored_size();
    let opens_as_miniplayer = app.invoke_miniplayer_mounted();
    *held().lock() = hold_at_launch(full_player, opens_as_miniplayer);
}

/// The hold a launch starts with, the full player saved beside a miniplayer close if there is one.
///
/// A size that opens the miniplayer with none saved, as a hand-edited one can, holds the first
/// launch's placement: holding nothing would leave `hold_full_player` only the miniplayer's own.
fn hold_at_launch(
    saved: Option<FullPlayerGeometry>,
    opens_as_miniplayer: bool,
) -> Option<Placement> {
    saved
        .map(Placement::from_persisted)
        .or_else(|| opens_as_miniplayer.then(Placement::first_launch))
}

/// A geometry as the logical size and position Slint takes, the size floored against a corrupt
/// source producing a window too small to use.
fn logical_placement(
    geom: PersistedGeometry,
    floor: LogicalSize,
) -> (LogicalSize, LogicalPosition) {
    // Persisted geometry is `f64`, Slint's logical types `f32` — window pixel coordinates
    // are small integers, well inside `f32`'s exact-integer range.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "window pixel coordinates are small; f32 precision is sufficient"
    )]
    let (w, h, x, y) = (geom.width as f32, geom.height as f32, geom.x as f32, geom.y as f32);
    (LogicalSize::new(w.max(floor.width), h.max(floor.height)), LogicalPosition::new(x, y))
}

/// How long a window has to sit neither resized nor moved before its geometry is one the user kept
/// rather than one a drag passed through. A drag reports a reading per pointer event, far inside
/// this, so the placement before a drag's first reading is the one it started from.
const SETTLE: Duration = Duration::from_millis(300);

/// The geometry plus whether winit ever reported a real position.
#[derive(Debug, Clone, Copy)]
struct Placement {
    geom: PersistedGeometry,
    /// `true` once `outer_position()` has succeeded at least once. Stays `false` on
    /// Wayland, where a client may not read its own position, so [`snapshot_into`] skips
    /// persisting x/y rather than degrading `settings.json` to `0, 0`.
    position_known: bool,
}

impl Placement {
    /// A fresh install's window, left where the window server puts it.
    fn first_launch() -> Self {
        Self { geom: PersistedGeometry::fallback(), position_known: false }
    }

    fn from_persisted(full_player: FullPlayerGeometry) -> Self {
        let FullPlayerGeometry { width, height, position } = full_player;
        let WindowPosition { x, y } = position.unwrap_or(WindowPosition { x: 0.0, y: 0.0 });
        Self {
            geom: PersistedGeometry { width, height, x, y, maximized: false },
            position_known: position.is_some(),
        }
    }

    fn to_persisted(self) -> FullPlayerGeometry {
        let PersistedGeometry { width, height, x, y, .. } = self.geom;
        FullPlayerGeometry {
            width,
            height,
            position: self.position_known.then_some(WindowPosition { x, y }),
        }
    }
}

/// Live-mirror payload.
#[derive(Debug, Clone, Copy)]
struct LiveGeometry {
    current: Placement,
    recorded_at: Instant,
    /// The last placement that sat still for [`SETTLE`] before a reading moved it. **What entering
    /// the miniplayer holds, rather than `current`**: by then `current` is the drag that shrank the
    /// window into it, a size just past the threshold rather than one the user ever used.
    settled: Placement,
}

impl LiveGeometry {
    /// The mirror a window's first reading starts, before that reading is taken.
    fn first(seen: Placement, now: Instant) -> Self {
        let unplaced = Placement { position_known: false, ..seen };
        Self { current: unplaced, recorded_at: now, settled: unplaced }
    }

    /// Takes a reading, settling the placement it replaces if that one sat still for [`SETTLE`].
    fn observe(&mut self, seen: Placement, now: Instant) {
        if now.duration_since(self.recorded_at) >= SETTLE {
            self.settled = self.current;
        }
        self.recorded_at = now;

        let current = &mut self.current;
        current.geom.maximized = seen.geom.maximized;
        if seen.geom.maximized {
            return;
        }
        current.geom.width = seen.geom.width;
        current.geom.height = seen.geom.height;
        if seen.position_known {
            current.geom.x = seen.geom.x;
            current.geom.y = seen.geom.y;
            current.position_known = true;
        }
    }
}

/// Live in-memory mirror, fed by every winit `Resized` / `Moved` through [`record`].
/// `None` until the first fires — winit emits a synthetic `Resized` on first map, so it is
/// populated before the user can close the window.
static LIVE_GEOMETRY: OnceLock<Mutex<Option<LiveGeometry>>> = OnceLock::new();

fn live() -> &'static Mutex<Option<LiveGeometry>> {
    LIVE_GEOMETRY.get_or_init(|| Mutex::new(None))
}

/// The full player's placement while the miniplayer is up, `None` otherwise.
static HELD_FULL_PLAYER: OnceLock<Mutex<Option<Placement>>> = OnceLock::new();

fn held() -> &'static Mutex<Option<Placement>> {
    HELD_FULL_PLAYER.get_or_init(|| Mutex::new(None))
}

/// The last reading [`frame_allowance`] has a decoration state and a client size from, which is
/// what the next one is a step away from.
static LAST_FRAMING: OnceLock<Mutex<Option<WindowReading>>> = OnceLock::new();

fn last_framing() -> &'static Mutex<Option<WindowReading>> {
    LAST_FRAMING.get_or_init(|| Mutex::new(None))
}

/// The window state a `Resized` or `Moved` event consults more than once, read in one pass.
///
/// On X11 the client size and the maximized state are each a blocking round trip on the UI
/// thread, so the readers below share one reading rather than asking per answer.
#[derive(Debug, Clone, Copy)]
pub struct WindowReading {
    client: WinitPhysicalSize<u32>,
    scale: f64,
    maximized: bool,
    decorated: bool,
}

impl WindowReading {
    pub fn take(w: &WinitWindow) -> Self {
        Self {
            client: w.inner_size(),
            scale: w.scale_factor(),
            maximized: w.is_maximized(),
            decorated: w.is_decorated(),
        }
    }

    pub fn is_maximized(self) -> bool {
        self.maximized
    }
}

/// Update the live mirror from the current winit window state, called by the `Resized` and
/// `Moved` handlers while the winit window is still alive.
///
/// Skips everything while minimized, the maximized flag included: winit clears it on a Win32
/// minimize, so closing from the taskbar would otherwise persist an un-maximized, empty rect at
/// the parked position, which the next launch can only clamp and re-centre.
///
/// Skips size and position while maximized: winit's `inner_size` there is the maximized
/// screen size, and persisting it would clobber the user's real restore geometry. The
/// `maximized` flag itself is still recorded.
pub fn record(w: &WinitWindow, reading: WindowReading) {
    if is_minimized_client(reading.client) {
        return;
    }
    let WindowReading { client, scale, maximized, .. } = reading;
    let inner: WinitLogicalSize<f64> = client.to_logical(scale);
    let outer: Option<WinitLogicalPosition<f64>> =
        w.outer_position().ok().map(|p| p.to_logical(scale));
    let seen = Placement {
        geom: PersistedGeometry {
            width: inner.width,
            height: inner.height,
            x: outer.map_or(0.0, |p| p.x),
            y: outer.map_or(0.0, |p| p.y),
            maximized,
        },
        position_known: outer.is_some(),
    };

    let now = Instant::now();
    live().lock().get_or_insert_with(|| LiveGeometry::first(seen, now)).observe(seen, now);
}

/// Hold the placement the full player last settled at, the miniplayer having just taken over.
///
/// A hold already there is kept: it is the one [`restore`] brought back with a window relaunched as
/// the miniplayer, whose settled placement is the miniplayer itself.
pub fn hold_full_player() {
    let settled = live().lock().map(|entry| entry.settled);
    let mut held = held().lock();
    if held.is_none() {
        *held = settled;
    }
}

/// Let the held placement go, the full player being back.
pub fn release_full_player() {
    *held().lock() = None;
}

/// Size and place the window back where the full player was held, which leaves the miniplayer.
///
/// Placed only where winit ever reported a position, so Wayland keeps the compositor's placement.
/// Nothing held falls back to the first-launch geometry.
pub fn restore_full_player(app: &AppWindow) {
    let placement = (*held().lock()).unwrap_or_else(Placement::first_launch);
    let (size, position) = logical_placement(placement.geom, MIN_FULL_PLAYER);
    let window = app.window();
    window.set_size(with_returning_frame(app, size));
    if placement.position_known {
        window.set_position(position);
    }
}

/// The miniplayer's size that leaves the full player at `client` once a native titlebar's frame
/// is back: the step that frame took out of the client when it went, handed back.
///
/// Where a returning frame comes out of the window rather than the client the step is 0 and this is
/// the identity, which is the platform split this used to be written as two bodies for.
fn with_returning_frame(app: &AppWindow, client: LogicalSize) -> LogicalSize {
    let chrome = app.global::<melodia_ui::WindowChrome>();
    LogicalSize::new(
        client.width + chrome.get_frame_allowance_w(),
        client.height + chrome.get_frame_allowance_h(),
    )
}

/// Returns what the OS frame adds to the client area, in logical pixels, on the reading that
/// crossed a decoration change, and `None` on every other.
///
/// **Measured as the step rather than asked of the window, because the window server that most
/// needs asking cannot answer.** A Wayland compositor draws its decoration outside the surface and
/// tells the client nothing about it, so `outer_size` there reads the client straight back and a
/// query is zero on exactly the desktop whose client grows by a titlebar when the miniplayer drops
/// it. The miniplayer then decides against an edge short by that titlebar and bounces in and out of
/// the threshold for as long as the window sits near it. The step is what both readers want anyway,
/// and where a dropped frame comes out of the window rather than going to the client it is honestly
/// zero.
///
/// Records the reading and answers what that reading decides, [`super::resize_release`]'s shape:
/// split into a query and a command, a caller could ask and forget to record.
pub fn frame_allowance(reading: WindowReading) -> Option<WinitLogicalSize<f32>> {
    settled_client(reading)?;
    let before = last_framing().lock().replace(reading)?;
    step_across(before, reading)
}

/// [`frame_allowance`] past its lock read.
///
/// `None` wherever the two readings wear the same decoration, which is every reading a resize drag
/// delivers: measured across those, the step would be the drag's own motion and the exit edge would
/// follow the pointer.
fn step_across(before: WindowReading, now: WindowReading) -> Option<WinitLogicalSize<f32>> {
    if before.decorated == now.decorated {
        return None;
    }
    let (framed, bare) =
        if before.decorated { (before.client, now.client) } else { (now.client, before.client) };
    Some(step_between(framed, bare, now.scale))
}

/// Returns the part of the OS frame outside the edges it draws, in logical pixels: what the client
/// takes over when the frame drops, without the visible window having moved.
///
/// Win32's left, right and bottom are invisible resize borders, and its top is the caption the user
/// sees, so `top` is always zero. `None` wherever [`settled_client`] finds no window to measure, and
/// wherever there is no frame standing: undecorated, the outer rect *is* the client, so the margins
/// would read zero and replace the ones the native miniplayer insets itself by.
#[cfg(target_os = "windows")]
pub fn frame_margins(w: &WinitWindow, reading: WindowReading) -> Option<WinitLogicalInsets<f32>> {
    if !reading.decorated {
        return None;
    }
    let inner = settled_client(reading)?;
    let outer = ScreenRect { at: w.outer_position().ok()?, size: w.outer_size() };
    let client = ScreenRect { at: w.inner_position().ok()?, size: inner };
    Some(margins_between(outer, client, reading.scale))
}

/// No invisible frame to hand over off Win32: X11 takes a dropped frame out of the window rather
/// than giving it to the client, and a macOS or Wayland frame has no undrawn edge beside it.
#[cfg(not(target_os = "windows"))]
pub fn frame_margins(_: &WinitWindow, _: WindowReading) -> Option<WinitLogicalInsets<f32>> {
    None
}

/// The client size while the window is one a frame reading may be taken against at all.
///
/// `None` while maximized, where a WM may strip the frame without the window ever learning it lost
/// one; and while minimized, the client then describing no window to measure.
fn settled_client(reading: WindowReading) -> Option<WinitPhysicalSize<u32>> {
    let settled = !reading.maximized && !is_minimized_client(reading.client);
    settled.then_some(reading.client)
}

/// Whether a client size is Win32's reading of a minimized window: empty, inside a small outer
/// rect parked at −32000, −32000, so nothing measured off it describes the window the user will
/// restore. Read off the size rather than asked of `is_minimized`, which costs X11 a round trip
/// on every resize and move event these run for.
fn is_minimized_client(client: WinitPhysicalSize<u32>) -> bool {
    client.width == 0 || client.height == 0
}

/// What the frame between a framed client and the bare one that replaced it was worth, in logical
/// pixels.
///
/// Floored at zero rather than wrapped: a wrapped `u32` is a step wider than any window, and the
/// exit edge would put the miniplayer past every size the window can be dragged to.
fn step_between(
    framed: WinitPhysicalSize<u32>,
    bare: WinitPhysicalSize<u32>,
    scale: f64,
) -> WinitLogicalSize<f32> {
    WinitPhysicalSize::new(
        bare.width.saturating_sub(framed.width),
        bare.height.saturating_sub(framed.height),
    )
    .to_logical(scale)
}

/// A rectangle on screen in physical pixels, winit reporting its corner and its size apart.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct ScreenRect {
    at: WinitPhysicalPosition<i32>,
    size: WinitPhysicalSize<u32>,
}

#[cfg(target_os = "windows")]
impl ScreenRect {
    fn left(self) -> i64 {
        i64::from(self.at.x)
    }

    fn right(self) -> i64 {
        i64::from(self.at.x) + i64::from(self.size.width)
    }

    fn bottom(self) -> i64 {
        i64::from(self.at.y) + i64::from(self.size.height)
    }
}

#[cfg(target_os = "windows")]
fn margins_between(outer: ScreenRect, client: ScreenRect, scale: f64) -> WinitLogicalInsets<f32> {
    WinitPhysicalInsets::new(
        0,
        gap(outer.left(), client.left()),
        gap(client.bottom(), outer.bottom()),
        gap(client.right(), outer.right()),
    )
    .to_logical(scale)
}

/// The distance from `near` to `far`, floored at zero rather than wrapped: a wrapped `u32` is a
/// margin wider than any window, and the miniplayer would inset itself out of existence.
#[cfg(target_os = "windows")]
fn gap(near: i64, far: i64) -> u32 {
    u32::try_from(far - near).unwrap_or(0)
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
pub fn ensure_on_screen(w: &WinitWindow, reading: WindowReading) {
    static DONE: Once = Once::new();
    let mut first = false;
    DONE.call_once(|| first = true);
    if !first {
        return;
    }

    if reading.is_maximized() {
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
///
/// A close from the miniplayer persists the miniplayer as the window and the held full player
/// beside it, which is what tells the next launch the restore button has somewhere to go.
pub fn snapshot_into(settings: &mut SettingsData) {
    let Some(mirror) = *live().lock() else {
        return;
    };
    write_snapshot(settings, mirror, *held().lock());
}

/// [`snapshot_into`] past its two lock reads.
fn write_snapshot(
    settings: &mut SettingsData,
    mirror: LiveGeometry,
    full_player: Option<Placement>,
) {
    settings.full_player_geometry = full_player.map(Placement::to_persisted);
    let Placement { geom, position_known } = mirror.current;
    settings.window.is_maximized = geom.maximized;
    if !geom.maximized {
        settings.window_width = geom.width;
        settings.window_height = geom.height;
        // Only when winit actually reported one — never on Wayland.
        if position_known {
            settings.window_x = geom.x;
            settings.window_y = geom.y;
        }
    }
}

#[cfg(test)]
#[path = "tests/geometry_tests.rs"]
mod tests;
