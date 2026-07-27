//! Winit `WindowEvent` filter installed by `window_chrome::install`.
//!
//! Subscribes to winit window events for five reasons:
//!
//! 1. **`MouseInput { Pressed, Left }`** when the cursor is over the
//!    titlebar drag area: start an OS-level window move and return
//!    `PreventDefault` so Slint never sees the press. Bypasses the
//!    TouchArea-grab issue described in the parent module's docs.
//! 2. **`MouseInput { Pressed, Back | Forward }`** (the side-buttons
//!    most pointing devices expose as Mouse-4 / Mouse-5): drive the
//!    browser-style nav history in [`crate::ui::nav_history`]. Gated
//!    on no overlay being open (NP / Queue / Dialog) so the buttons
//!    don't fight the overlay's own input context.
//! 3. **`Resized`**: re-read `is_maximized` so the maximize/restore
//!    icon stays in sync with Win+↑ / window-tile shortcuts that
//!    bypass our buttons.
//! 4. **`Focused(bool)`**: mirror the OS focus state into
//!    `Theme.window-focused` — always, unconditionally — and use the
//!    transition as the cue to reconcile the tray-bridge's visibility
//!    shadow, raising it on focus and asking the OS about a minimize on
//!    focus loss (see [`schedule_minimize_probe`]). Two consumers
//!    today: the KDE "Match Unfocused Window Background" tint (sidebar
//!    and now-playing bar bindings gate themselves on
//!    `Settings.match-unfocused-bg && Theme.use-native-titlebar
//!    && !Theme.window-focused`) and the `PopupWindow` auto-dismiss
//!    handlers — each popup body mounts a `FocusLossWatcher` inside an
//!    `if popup-is-open` gate (keyed on `PopupHighlight.id` for
//!    singleton popups or `PopupHighlight.row-ctx-id` for the
//!    track-list row context menu) so only the currently-open popup
//!    has a live watcher. Any future feature needing raw OS focus can
//!    subscribe directly.
//! 5. **`DroppedFile` / `HoveredFile` / `HoveredFileCancelled`**:
//!    Slint 1.16 has no cross-app `DnD` API, so the winit layer is the
//!    only path through which the platform reports file drops to us.
//!    Drops route through [`super::drop_coalescer::schedule_drop_flush`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use slint::winit_030::winit::window::Window as WinitWindow;

use crate::state::AppState;
use crate::{AppWindow, CompositeScroll, PlaylistDetail, PopupHighlight, Queue, Theme};

use super::{drop_coalescer, geometry};

/// How long to wait after a focus loss before asking whether it was a minimize.
///
/// The window manager sets the minimized state and drops focus at about the same
/// time, and not reliably in that order — X11 publishes `_NET_WM_STATE_HIDDEN`
/// on its own event stream, Win32 sends `WM_ACTIVATE` and `WM_SIZE`
/// independently. Asking a moment later sidesteps the ordering question
/// entirely, for the cost of one deferred timer per focus loss.
const MINIMIZE_PROBE_DELAY: Duration = Duration::from_millis(250);

/// Ask the OS whether a focus loss was really a minimize, and drop the
/// visibility shadow if it was.
///
/// This is what brings a minimize we didn't drive — the native titlebar's
/// button, a taskbar click, "show desktop" — to the same gates our own minimize
/// button reaches: the visualizer stops drawing for a window nobody can see, and
/// the tray "Show / Hide" toggle restores rather than re-hiding an already
/// hidden window.
///
/// `winit::Window::is_minimized` answers `None` on Wayland — the protocol never
/// tells a client it was minimized — so that case is left exactly as it was, and
/// `ui::visualizer::pulse` is what covers it.
fn schedule_minimize_probe(weak: slint::Weak<AppWindow>) {
    slint::Timer::single_shot(MINIMIZE_PROBE_DELAY, move || {
        let Some(ui) = weak.upgrade() else { return };
        let minimized = ui
            .window()
            .with_winit_window(WinitWindow::is_minimized)
            .flatten();
        if minimized == Some(true) {
            crate::ui::tray_bridge::set_window_visible(&ui, false);
        }
    });
}

pub(super) fn install(app: &AppWindow, state: &AppState, drag_hover: Arc<AtomicBool>) {
    let weak = app.as_weak();
    let state = state.clone();
    app.window().on_winit_window_event(move |w, event| {
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if drag_hover.load(Ordering::Relaxed) => {
                // Hand the press to the OS for a window move. Errors
                // are non-fatal: an unsupported backend (Web/Android)
                // just leaves the cursor where it is — desktop always
                // succeeds on Wayland/X11/Win32/macOS. The closure's
                // `w` is `&slint::Window`; reach the underlying winit
                // window via the accessor.
                let _ = w.with_winit_window(|ww| {
                    if let Err(e) = ww.drag_window() {
                        log::debug!("drag_window unsupported on this platform: {e}");
                    }
                });
                slint::winit_030::EventResult::PreventDefault
            }
            // Mouse-4 / Mouse-5 → browser-style back / forward through
            // the in-memory nav history. Sits above the `Released`
            // cleanup arm because we want PreventDefault on the press,
            // and the press needs its own arm to do so. Overlay gates
            // (NP / Queue / Dialog) live inside `nav_history::replay`
            // via `overlay_open` — if any overlay is up we `Propagate`
            // so Slint can give it its own input semantics (Esc/F
            // already close NP and Queue, and the dialog backdrop
            // catches the dismissing click).
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: button @ (MouseButton::Back | MouseButton::Forward),
                ..
            } => {
                let Some(ui) = weak.upgrade() else {
                    return slint::winit_030::EventResult::Propagate;
                };
                if crate::ui::nav_history::overlay_open(&ui) {
                    return slint::winit_030::EventResult::Propagate;
                }
                let going_back = matches!(button, MouseButton::Back);
                crate::ui::nav_history::replay(&state, &ui, going_back);
                slint::winit_030::EventResult::PreventDefault
            }
            // Popup-trigger highlight cleanup. Slint 1.16's `PopupWindow`
            // has no `closed` callback, and while a popup is open the
            // dispatcher stops routing mouse events to main-window items
            // (`i-slint-core::window::process_mouse_input`), so an
            // AppWindow-level scrim TouchArea can't catch the dismissing
            // click. Clearing on Release works because by then the press
            // has already triggered Slint's `close-on-click-outside` for
            // the popup; popup-internal interactive areas that should
            // KEEP the highlight after release (volume slider, column-
            // toggle rows) re-set `PopupHighlight.id` in their own
            // `pointer-event(up)` handler — that fires *after* this
            // filter, so the final per-frame value is the re-set one.
            // `row-ctx-id` rides the same release event: it's set on
            // right-click by `TrackListRowItem` to scope the
            // `FocusLossWatcher` to the one open row, and a click-outside
            // (or click-inside that doesn't re-set it) needs the same
            // clear so the gate doesn't stick on a stale row id. Both
            // writes are gated on "value differs from sentinel" so we
            // don't churn the property on every random click.
            WindowEvent::MouseInput { state: ElementState::Released, .. } => {
                // Synchronous `weak.upgrade()` (not `upgrade_in_event_loop`):
                // the winit filter runs on the UI thread already, and the
                // ordering matters — popup-internal `pointer-event(up)`
                // handlers re-set `id` *after* this filter returns, so we
                // need the clear to land before they fire (otherwise the
                // deferred clear lands last and wipes the re-set).
                if let Some(ui) = weak.upgrade() {
                    let ph = ui.global::<PopupHighlight>();
                    if !ph.get_id().is_empty() {
                        ph.set_id("".into());
                    }
                    if ph.get_row_ctx_id() != -1 {
                        ph.set_row_ctx_id(-1);
                    }
                }
                slint::winit_030::EventResult::Propagate
            }
            WindowEvent::Resized(_) => {
                // Capture size + maximize-state into the live mirror
                // while the winit window is still alive. Shutdown
                // reads from the mirror because `with_winit_window`
                // returns `None` after `app.run()` returns.
                let maximized = w
                    .with_winit_window(|ww| {
                        geometry::record(ww);
                        // Off-screen recovery; one-shot, runs on the
                        // first (synthetic, post-map) `Resized` only.
                        geometry::ensure_on_screen(ww);
                        ww.is_maximized()
                    })
                    .unwrap_or(false);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<crate::WindowChrome>().set_is_maximized(maximized);
                });
                slint::winit_030::EventResult::Propagate
            }
            // Position changes (title-bar drag, Win+arrow tile, etc.).
            // X11/Win32/macOS deliver these; Wayland clients don't know
            // their own position so `Moved` is rare/never there — that's
            // fine, position restore is a no-op on Wayland anyway.
            WindowEvent::Moved(_) => {
                let _ = w.with_winit_window(geometry::record);
                slint::winit_030::EventResult::Propagate
            }
            WindowEvent::Focused(focused) => {
                let focused = *focused;
                // `Focused(false)` on its own is ambiguous — another window
                // got focus, not necessarily that we're hidden — so ask the
                // OS which it was. See `schedule_minimize_probe` for why the
                // question is deferred rather than asked here. Only worth
                // asking while we still believe the window is up: a tray hide
                // and our own minimize button both lower the shadow first, and
                // on X11 the answer costs a round-trip on the UI thread.
                if !focused && crate::ui::tray_bridge::is_window_visible() {
                    schedule_minimize_probe(weak.clone());
                }
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<Theme>().set_window_focused(focused);
                    // Focus implies the surface is visible and the user is
                    // interacting with it — covers the taskbar-restore path
                    // where the WM un-minimizes the window without going
                    // through any of our callbacks, so the tray-bridge shadow
                    // would otherwise stay stuck at the post-minimize `false`.
                    if focused {
                        crate::ui::tray_bridge::set_window_visible(&ui, true);
                    }
                });
                slint::winit_030::EventResult::Propagate
            }
            // OS-level file drop. The coalescer downstream batches
            // multi-file drops and re-checks both the queue-sheet gate
            // and the playlist-detail gate at flush time — see
            // [`drop_coalescer::schedule_drop_flush`]. Wayland delivery
            // depends on the vendored winit patch (see `Cargo.toml`'s
            // `[patch.crates-io]` block).
            WindowEvent::DroppedFile(path) => {
                drop_coalescer::schedule_drop_flush(&state, path.clone());
                // Drag is over — clear both hover banners regardless of
                // which gate accepted. Otherwise the banner would stick
                // until the next pointer event.
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.global::<Queue>().set_is_drop_hovered(false);
                    ui.global::<PlaylistDetail>().set_is_drop_hovered(false);
                });
                slint::winit_030::EventResult::Propagate
            }
            // OS-level file hover. Drives either the "Drop to add to
            // queue" banner inside the queue sheet OR the
            // "Drop files to add to this playlist" banner inside
            // Playlist Detail. Queue takes precedence — when the queue
            // sheet is open, only its banner shows; otherwise the
            // playlist banner shows (gated on `playlist-id >= 0` in
            // markup). The flush-side routing in `drop_coalescer`
            // applies the same precedence, so the indicator always
            // matches where the drop will actually land.
            WindowEvent::HoveredFile(_) => {
                let queue_open = drop_coalescer::is_queue_sheet_open();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<Queue>().set_is_drop_hovered(true);
                    // Only paint the playlist banner when the queue
                    // sheet ISN'T the active drop target.
                    ui.global::<PlaylistDetail>().set_is_drop_hovered(!queue_open);
                });
                slint::winit_030::EventResult::Propagate
            }
            WindowEvent::HoveredFileCancelled => {
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.global::<Queue>().set_is_drop_hovered(false);
                    ui.global::<PlaylistDetail>().set_is_drop_hovered(false);
                });
                slint::winit_030::EventResult::Propagate
            }
            // OS / WM close path (native-titlebar X button, Alt+F4) — the
            // custom-titlebar close button instead routes through
            // `WindowChrome.on_close_window` in `controls.rs`. Always
            // `PreventDefault` and act explicitly: the event loop runs via
            // `run_event_loop_until_quit`, so a non-tray close must call
            // `quit_event_loop()` itself — `CloseRequested` no longer
            // auto-quits. Close-to-tray instead hides the window; the hide is
            // deferred via `invoke_from_event_loop` because doing it inline
            // (we are inside a `WindowEvent` dispatch) trips Slint's
            // "references to the window still exist" warning.
            // `should_hide_to_tray` is false when no tray is active, so this
            // can't strand the user.
            WindowEvent::CloseRequested => {
                if crate::ui::tray_bridge::should_hide_to_tray() {
                    if let Err(e) = weak.upgrade_in_event_loop(|ui| {
                        crate::ui::tray_bridge::hide_window(&ui);
                    }) {
                        log::warn!("close-to-tray: schedule hide: {e}");
                    }
                } else if let Err(e) = slint::quit_event_loop() {
                    log::warn!("close-window: quit_event_loop: {e}");
                }
                slint::winit_030::EventResult::PreventDefault
            }
            // Composite-view wheel routing. Favorites / Recently Played /
            // Artist Detail / Browse stack a strip section above a height-
            // capped (virtualized) `TrackList` inside one outer `ScrollView`.
            // Slint's inner `ListView` swallows the vertical wheel at its top
            // edge instead of bubbling to the outer scroller, so the strips/hero
            // can't be wheeled back once scrolled away (see `CompositeScroll` in
            // globals.slint for the full root-cause note). There is no
            // Slint-native seam to intercept the wheel without eating row
            // clicks, so we do it here: only vertical-dominant wheel over a live
            // composite region is taken over and fed into that view's existing
            // composite scroll math. Horizontal-dominant wheel (column pan,
            // strip carousels), the sidebar, and every non-composite view stay
            // native. Delta conversion mirrors the Slint winit backend exactly
            // (`event_loop.rs`: line delta × 60, pixel delta → logical).
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(lx, ly) => (lx * 60.0, ly * 60.0),
                    MouseScrollDelta::PixelDelta(p) => {
                        let l = p.to_logical::<f32>(f64::from(w.scale_factor()));
                        (l.x, l.y)
                    }
                };
                let Some(ui) = weak.upgrade() else {
                    return slint::winit_030::EventResult::Propagate;
                };
                // Don't hijack the wheel while an overlay (Queue sheet / Dialog /
                // Now Playing) is up: `hovered` can be stale-true if the overlay
                // opened over a composite region without a pointer move, since
                // Slint only clears `has-hover` on the next `MouseEvent::Exit`
                // (one pointer event later). Mirrors the Mouse-4/5 arm's own
                // overlay gate.
                let cs = ui.global::<CompositeScroll>();
                if cs.get_hovered()
                    && !crate::ui::nav_history::overlay_open(&ui)
                    && dy != 0.0
                    && dy.abs() >= dx.abs()
                {
                    // Accumulate rather than overwrite: Slint fires the mounted
                    // view's `changed wheel-tick` handler at most once per loop
                    // iteration, so several wheel events landing before a re-eval
                    // would otherwise collapse to just the last delta (under-scroll
                    // on fast flicks). The view zeroes `wheel-dy` after applying, so
                    // this only ever sums deltas within one un-applied frame.
                    cs.set_wheel_dy(cs.get_wheel_dy() + dy);
                    cs.set_wheel_tick(cs.get_wheel_tick().wrapping_add(1));
                    return slint::winit_030::EventResult::PreventDefault;
                }
                slint::winit_030::EventResult::Propagate
            }
            _ => slint::winit_030::EventResult::Propagate,
        }
    });
}
