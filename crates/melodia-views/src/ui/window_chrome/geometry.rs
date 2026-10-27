//! Window geometry: restored before `app.show()`, mirrored from winit while the window lives,
//! snapshotted at exit.
//!
//! Restore goes through `slint::Window::set_size`/`set_position` rather than the attributes hook:
//! `set_size` sets the backend's `has_explicit_size`, which stops Slint snapping the window to its
//! preferred size at first show, and `with_inner_size` doesn't. Both land before the winit window
//! exists, so a KDE window rule still wins. Maximized has no `slint::Window` API, so `main.rs`'s
//! attributes hook applies it. Wayland ignores `set_position`.
//!
//! The window is gone by `shutdown::save_state_on_exit`, so the winit filter feeds [`record`] a
//! mirror for [`snapshot_into`] to read.
//!
//! The miniplayer is a window size, so the full player's placement is held across it
//! ([`hold_full_player`], [`restore_full_player`]) and persisted beside a close from it.

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

/// Window geometry as `settings.json` holds it.
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

    /// The default settings' geometry, for a `settings.json` that couldn't be read.
    pub fn fallback() -> Self {
        Self::from_settings(&SettingsData::default())
    }
}

/// Restores the persisted size and position, `WindowChrome.is-maximized` and the full player held
/// behind a miniplayer close. Runs between `AppWindow::new()` and `app.show()`.
pub fn restore(app: &AppWindow, geom: PersistedGeometry, full_player: Option<FullPlayerGeometry>) {
    let mini = app.global::<MiniPlayer>();
    let floor = LogicalSize::new(mini.get_window_min_width(), mini.get_window_min_height());
    let (size, position) = logical_placement(geom, floor);
    let window = app.window();
    window.set_size(size);
    window.set_position(position);

    // So a window restored maximized paints square from the first frame.
    app.global::<melodia_ui::WindowChrome>().set_is_maximized(geom.maximized);

    // After `set_size`, which is what hands the switch the size it decides on.
    app.invoke_adopt_restored_size();
    let opens_as_miniplayer = app.invoke_miniplayer_mounted();
    *held().lock() = hold_at_launch(full_player, opens_as_miniplayer);
}

/// The hold a launch starts with. A miniplayer-sized launch with nothing saved, as a hand-edited
/// size can be, holds the first launch's placement: holding nothing would leave
/// [`hold_full_player`] only the miniplayer's own.
fn hold_at_launch(
    saved: Option<FullPlayerGeometry>,
    opens_as_miniplayer: bool,
) -> Option<Placement> {
    saved
        .map(Placement::from_persisted)
        .or_else(|| opens_as_miniplayer.then(Placement::first_launch))
}

/// The size and position Slint takes, the size floored so a corrupt source can't leave a window
/// too small to use.
fn logical_placement(
    geom: PersistedGeometry,
    floor: LogicalSize,
) -> (LogicalSize, LogicalPosition) {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "window pixel coordinates are small; f32 precision is sufficient"
    )]
    let (w, h, x, y) = (geom.width as f32, geom.height as f32, geom.x as f32, geom.y as f32);
    (LogicalSize::new(w.max(floor.width), h.max(floor.height)), LogicalPosition::new(x, y))
}

/// How long a placement has to sit still to count as one the user kept. A drag reports far inside
/// this, so the placement before its first reading is the one it started from.
const SETTLE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy)]
struct Placement {
    geom: PersistedGeometry,
    /// Stays `false` on Wayland, which won't tell a client its position, so [`snapshot_into`]
    /// leaves x/y alone rather than writing `0, 0`.
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

#[derive(Debug, Clone, Copy)]
struct LiveGeometry {
    current: Placement,
    recorded_at: Instant,
    /// The last placement that sat still for [`SETTLE`]. Entering the miniplayer holds this rather
    /// than `current`, which by then is the drag that shrank the window into it.
    settled: Placement,
}

impl LiveGeometry {
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

/// Fed by [`record`]. `None` until the first reading, which winit's synthetic first-map `Resized`
/// delivers before a close is possible.
static LIVE_GEOMETRY: OnceLock<Mutex<Option<LiveGeometry>>> = OnceLock::new();

fn live() -> &'static Mutex<Option<LiveGeometry>> {
    LIVE_GEOMETRY.get_or_init(|| Mutex::new(None))
}

/// The full player's placement while the miniplayer is up, `None` otherwise.
static HELD_FULL_PLAYER: OnceLock<Mutex<Option<Placement>>> = OnceLock::new();

fn held() -> &'static Mutex<Option<Placement>> {
    HELD_FULL_PLAYER.get_or_init(|| Mutex::new(None))
}

/// The reading [`frame_allowance`] measures the next step from.
static LAST_FRAMING: OnceLock<Mutex<Option<WindowReading>>> = OnceLock::new();

fn last_framing() -> &'static Mutex<Option<WindowReading>> {
    LAST_FRAMING.get_or_init(|| Mutex::new(None))
}

/// When the window last announced gaining or losing its OS frame, cleared once a step is measured
/// across it.
static FRAME_CHANGED_AT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

fn frame_changed_at() -> &'static Mutex<Option<Instant>> {
    FRAME_CHANGED_AT.get_or_init(|| Mutex::new(None))
}

/// Stamps `app-window.slint`'s decoration changes, the one thing [`frame_allowance`] can't see: no
/// winit event announces one on the desktops that answer it with no size at all.
pub fn wire_frame_changes(app: &AppWindow) {
    app.global::<melodia_ui::WindowChrome>()
        .on_frame_changed(|| *frame_changed_at().lock() = Some(Instant::now()));
}

/// The window state a `Resized` or `Moved` event reads more than once, taken in one pass: on X11
/// each answer is a blocking round trip on the UI thread.
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

/// Updates the live mirror.
///
/// Skips a minimized window entirely: winit clears the maximized flag on a Win32 minimize, so a
/// close from the taskbar would persist an empty rect at the parked position. While maximized only
/// the flag is recorded, `inner_size` being the screen's rather than the user's restore size.
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

/// Holds the placement the full player last settled at, the miniplayer having taken over. A hold
/// already there is kept: [`restore`] seeded it for a relaunched miniplayer, whose settled
/// placement is the miniplayer itself.
pub fn hold_full_player() {
    let settled = live().lock().map(|entry| entry.settled);
    let mut held = held().lock();
    if held.is_none() {
        *held = settled;
    }
}

pub fn release_full_player() {
    *held().lock() = None;
}

/// Sizes and places the window where the full player was held, leaving the miniplayer. Placed only
/// where winit ever reported a position, so Wayland keeps the compositor's placement.
pub fn restore_full_player(app: &AppWindow) {
    let placement = (*held().lock()).unwrap_or_else(Placement::first_launch);
    let (size, position) = logical_placement(placement.geom, MIN_FULL_PLAYER);
    let window = app.window();
    window.set_size(with_returning_frame(app, size));
    if placement.position_known {
        window.set_position(position);
    }
}

/// The size that leaves the full player at `client` once a native titlebar's frame is back: the
/// step that frame took out of the client when it went, handed back.
fn with_returning_frame(app: &AppWindow, client: LogicalSize) -> LogicalSize {
    let chrome = app.global::<melodia_ui::WindowChrome>();
    LogicalSize::new(
        client.width + chrome.get_frame_allowance_w(),
        client.height + chrome.get_frame_allowance_h(),
    )
}

/// How long after [`wire_frame_changes`]' stamp a reading can still be the decoration change's.
///
/// Unbounded, a desktop answering a dropped frame with no size leaves the next resize of any kind
/// looking like the crossing, and the leave's own resize reads as a frame. `KWin`'s
/// `CONFIGURE_GRACE` in [`super::resize_release`] is the same budget for the same round trip.
const FRAME_STEP_GRACE: Duration = Duration::from_millis(250);

/// Returns what the OS frame adds to the client area, in logical pixels, on a reading
/// [`step_across`] can pin on a decoration change, and `None` on every other.
///
/// **Measured as the step rather than asked of the window**: a Wayland compositor draws its
/// decoration outside the surface and tells the client nothing, so a query is zero on exactly the
/// desktop whose client grows by a titlebar when the miniplayer drops it, and the miniplayer then
/// bounces across its threshold. Where a dropped frame comes out of the window rather than going to
/// the client, the step is honestly zero.
///
/// Records the reading as it answers, so no caller can ask and forget to record.
pub fn frame_allowance(reading: WindowReading) -> Option<WinitLogicalSize<f32>> {
    measurable_client(reading)?;
    let since_flip = frame_changed_at().lock().map(|at| at.elapsed());
    let before = last_framing().lock().replace(reading)?;
    let step = step_across(before, reading, since_flip)?;
    *frame_changed_at().lock() = None;
    Some(step)
}

/// [`frame_allowance`] past its lock reads. `None` across two readings wearing the same decoration,
/// as every reading in a resize drag does (the step would be the drag's own motion), past
/// [`FRAME_STEP_GRACE`], and across a scale change, the two sizes then being in different units.
fn step_across(
    before: WindowReading,
    now: WindowReading,
    since_flip: Option<Duration>,
) -> Option<WinitLogicalSize<f32>> {
    if before.decorated == now.decorated {
        return None;
    }
    if since_flip.is_none_or(|since| since > FRAME_STEP_GRACE) {
        return None;
    }
    // Bit equality, not a tolerance: the factor is one exact `f64` until a monitor change.
    if before.scale.to_bits() != now.scale.to_bits() {
        return None;
    }
    let (framed, bare) =
        if before.decorated { (before.client, now.client) } else { (now.client, before.client) };
    Some(step_between(framed, bare, now.scale))
}

/// Returns the part of the OS frame outside the edges it draws, in logical pixels: what the client
/// takes over when the frame drops, the visible window not moving.
///
/// Win32's left, right and bottom are invisible resize borders and its top is the visible caption,
/// so `top` is zero. `None` without a frame standing: undecorated, the outer rect is the client, and
/// zero margins would replace the ones the native miniplayer insets itself by.
#[cfg(target_os = "windows")]
pub fn frame_margins(w: &WinitWindow, reading: WindowReading) -> Option<WinitLogicalInsets<f32>> {
    if !reading.decorated {
        return None;
    }
    let inner = measurable_client(reading)?;
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

/// The client size while a frame reading means anything: not while maximized, where a WM may strip
/// the frame unannounced, nor while minimized.
fn measurable_client(reading: WindowReading) -> Option<WinitPhysicalSize<u32>> {
    let measurable = !reading.maximized && !is_minimized_client(reading.client);
    measurable.then_some(reading.client)
}

/// Win32's reading of a minimized window, an empty client parked at −32000. Read off the size
/// because `is_minimized` costs X11 a round trip per event.
fn is_minimized_client(client: WinitPhysicalSize<u32>) -> bool {
    client.width == 0 || client.height == 0
}

/// What the frame between a framed client and the bare one replacing it was worth, floored at zero:
/// a wrapped `u32` would put the exit edge past any size the window can be dragged to.
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

fn dim_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Re-centres a restored window that overlaps no connected monitor, the custom titlebar offering no
/// native "move window". Runs once, from the first `Resized`, by which point winit's synthetic
/// first-map event has populated `available_monitors()`. Wayland and a maximized window are left to
/// the compositor.
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
        return;
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

/// Writes the live mirror into `settings`, untouched if no reading ever arrived. A close from the
/// miniplayer persists it as the window and the held full player beside it, which is where the
/// next launch's restore button goes.
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
        if position_known {
            settings.window_x = geom.x;
            settings.window_y = geom.y;
        }
    }
}

#[cfg(test)]
#[path = "tests/geometry_tests.rs"]
mod tests;
