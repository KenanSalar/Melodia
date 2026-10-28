---
paths:
  - crates/melodia-views/src/ui/window_chrome/**/*.rs
  - crates/melodia-views/src/ui/shell/tray_bridge.rs
  - crates/melodia-views/src/ui/shell/event_sink.rs
  - crates/melodia-platform/src/services/platform/tray/**/*.rs
  - crates/melodia-integrations/src/services/integrations/media_controls/**/*.rs
  - crates/melodia-platform/src/services/platform/always_on_top/**/*.rs
  - crates/melodia-platform/src/services/platform/dwm_titlebar.rs
  - crates/melodia-views/src/ui/appearance/theme_paint.rs
  - crates/melodia/src/main.rs
  - crates/melodia/src/shutdown.rs
  - crates/melodia/src/boot/**/*.rs
  - crates/melodia-ui/ui/app-window.slint
  - crates/melodia-ui/ui/components/custom-titlebar.slint
  - crates/melodia-ui/ui/components/caption-buttons.slint
  - crates/melodia-ui/ui/components/macos-titlebar-cluster.slint
  - crates/melodia-ui/ui/components/macos-traffic-light.slint
  - crates/melodia-ui/ui/layout/resize-ring.slint
  - crates/melodia-ui/ui/views/settings/window-chrome-section.slint
  - crates/melodia-ui/ui/views/settings/mini-player-section.slint
  - crates/melodia-ui/ui/views/mini-player.slint
  - crates/melodia-ui/ui/components/mini-player/*.slint
  - crates/melodia-views/src/ui/appearance/window_settings.rs
---

# The desktop shell: window chrome, tray, media keys

**Slint's window API is a cache and winit is the truth**, and what the OS owns attaches late, or
not at all, on at least one platform. Each module under `ui/window_chrome/` argues its own design;
this file holds what couples Rust to the `.slint` tree, or breaks from outside its owner.

## Window chrome

- **Window controls go through winit, never `slint::Window::set_minimized`/`set_maximized`**,
  which forward only when a cached flag differs, a flag Wayland never clears
  (`window_chrome::controls`).
- **Nothing done through `with_winit_window` reaches the OS before the event loop builds the
  window**, the accessor answering `None`, which a `let _ =` swallows. Await `winit_window()`, as
  `window_chrome::seed_always_on_top` does, or ride the attributes hook, as maximized does.
- **A drag region or `ResizeRing` reports its hover and nothing else**; the winit filter takes the
  press, resize first. So no `double-clicked` on a drag region fires (`window_chrome::drag_region`
  reads the titlebar's), and the `Window`'s `resize-border-width` stays unbound, Slint's handler
  giving a corner a square with no diagonal (`frameless_tests.rs`).
- **A caption style is a chip index in Slint and a name in `settings.json`**
  (`window_settings::{idx_for, style_for}`), each chip group's `options` order being the index. A
  new one is a variant, both arms, a chip in `window-chrome-section.slint` and in
  `mini-player-section.slint`, a case in `custom-titlebar.slint`'s and `MiniCaptions`' index test
  and a row in `window_settings_tests.rs`' `CHIPS`. A drawn style rides `CaptionStyle` and adds no
  branch.
- **A Win32 drag parks every Slint `Timer` and `changed` handler**, which
  `window_chrome::parked_loop` pumps from the filter's `RedrawRequested` and `Moved` arms. A new
  arm a drag can reach is a third pump site.
- **The native titlebar, the tray icon and the backdrop style restart rather than apply**, boot
  being each one's only reader: a `DialogKind.restart-*`, a `WindowChrome` callback and
  `window_chrome::controls`' `restart_toggle`. The backdrop is read ahead of `install_views`,
  whose artwork tiers decide at construction (`ui_setup_tests.rs`).
- **Every respawn goes through `window_chrome::request_respawn_and_quit`**, which can refuse and
  argues why the check sits before the exit. Its statics are private, so there is no second
  spelling.
- **What the OS changes unannounced is re-read in the winit `Focused` arm**: the Windows app mode,
  the accent a window border takes, the tray's surface. A new OS-owned colour joins that arm.

## The native miniplayer's frame

Under the native titlebar the miniplayer drops the frame (`app-window.slint`'s `frameless`, which
argues the timing). Win32, macOS and `KWin` keep the window's rect, so the client grows into the
frame it lost, and a missing link bounces a window parked near the threshold in and out:

- **The winit `Resized` arm measures the step** (`geometry::frame_allowance`), counted only just
  after `WindowChrome.frame-changed` announces a decoration change
  (`geometry::wire_frame_changes`). Unwired, the allowance stays zero; unbounded, the leave's own
  resize reads as a frame and the miniplayer never leaves.
- **`WindowChrome.frame-allowance-*` holds it**: `MiniPlayerSwitch` widens its exit edge by it, the
  root's minimum gives it back while a frame stands, `restore_full_player` hands it back.
- **Win32's invisible borders pass to the client too**: `geometry::frame_margins` →
  `WindowChrome.frame-margin-*` → the shell and overlays inset, the `resize-band` widened over them.
- **`geometry::restore` ends in `adopt-restored-size()`**, so a relaunched miniplayer maps knowing
  it is one. Without it, it is built framed, drops the frame a pass later and on Win32 grows by it
  every launch; Linux shows nothing.
- **The window floor is `MiniPlayer.window-min-*` alone**, read by the root and by `restore`.

`frameless_tests.rs` pins the Slint half, `window_chrome`'s `geometry_tests.rs` the Rust half.

## The shell and its edge

- **The shell is the window's only rounded clip, and holds everything that paints**, transparent
  outside it, opaque and square when maximized or framed (`rounded-shell`). Its clip is the
  corner's only antialiased pass (`slint-pitfalls.md`), so it has no `background`, the overlays
  mount inside and the outline is an `EdgeOutline` mounted last, each argued at its mount. A new
  element reaching the window's edge goes inside too.
- **The content panel is rounded by `RoundedFrame`, not a clip**, masking its corners in
  `WindowChrome.mantle`: right only while the shell's ground is all that shows around the panel.
- **The unfocused tint is one brush, `WindowChrome.mantle`**, which argues its terms, and every
  surface the chrome shows through reads it. One spelling the ternary, or `Theme.mantle`, stays
  dark on an unfocused KDE window.
- **On `KWin` the always-on-top pin is read back as well as written**, its keep-above button moving
  it unannounced: `always_on_top::kwin::watch_keep_above` reports, and
  `window_chrome::follow_reported_pin` mirrors into the button and the setting. GNOME and winit
  report nothing, so there a pin moved from the titlebar leaves the button behind.

## File drops

- **OS file drops ride the vendored winit fork and are routed in Rust**, Slint having no pointer
  during one (`winit/README.md`, which also says what retires it). Each target's
  `WindowRectReport` publishes its rect on `FileDrag`, and `window_chrome::file_drop` writes
  `FileDrag.target`, the one thing both banners read. **A report sits in no `if` but the full-UI
  branch**, any other being the `changed`-in-`if` panic (`slint-pitfalls.md`), so the playlist's
  target is the whole content panel (`crates/melodia/tests/file_drop.rs`). The content panel's
  report also feeds `WindowChrome.content-panel-width` to the grid cover tier.

## Tray and media keys

- **Windows SMTC attaches only once the window exists** (`MediaControlsHandle::new`), and boot
  re-emits the player state then, the inert handle having dropped every update.
- **The tray icon follows the surface it sits on, not the app**: on a light taskbar or Plasma panel
  `tray/light_taskbar.rs` paints it in `themes::BRAND_MARK_ON_LIGHT`, `theme.slint`'s `brand-mark`
  pair, the two pinned by `themes`' `registry_tests.rs`.
- **Hiding and showing go through `tray_bridge::hide_window` and `raise_window`**, the only paths
  that know `SAVED_WINDOW_GEOM`, and every visibility change through `set_window_visible`, whose
  shadow stands in for winit's `is_visible()`, `None` on Wayland.

## Shutdown

- **Force-exit.** `main()` ends in `std::process::exit(0)`, so whatever a destructor would have done
  is an explicit call ahead of it, argued at `main()`'s tail and in `shutdown.rs`: the output
  closes, the Windows and macOS tray drops (`tray_bridge::shutdown`) or its icon ghosts, the log
  flushes. `main_order_tests.rs` pins the output close and the log flush ahead of both ways out.
