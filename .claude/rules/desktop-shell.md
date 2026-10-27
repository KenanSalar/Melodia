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

# The desktop shell — window chrome, tray, media keys

The app's relationship with the OS window server and the desktop's notification/media surfaces.
The theme running through it: **Slint's window API is a cache, winit is the truth**, and anything
the OS owns has to be attached late or not at all on at least one platform.

## Window chrome

- **Window-control APIs go through winit, not Slint.** Slint's `set_minimized`/`set_maximized`
  cache stalls on Wayland: `WinitWindowAccessor::with_winit_window(|w| w.set_minimized(true))`.

- **Window dragging belongs at the winit layer.** `drag_window()` from Slint `pointer-event(down)`
  leaks the input grab. Each drag region's `TouchArea` reports its hover under its own name via
  `WindowChrome.drag-region-changed(DragRegion)`; `on_winit_window_event` intercepts
  `MouseInput { Pressed, Left }` over either → `drag_window()` → `PreventDefault`.
  **So no `double-clicked` on a drag region can fire**, Slint never seeing the press: the
  titlebar's double press is read in `window_chrome::drag_region`, and the miniplayer names itself
  so its own only moves.

- **Resizing a frameless window takes the same path, and the ring's hover is the whole geometry.**
  `ResizeRing` reports a `ResizeZone` via `WindowChrome.resize-zone-changed`, kept by
  `window_chrome::resize_grab`, and the filter's resize arm (`drag_resize_window`) is matched
  **before** the drag arm, since the miniplayer's drag region runs to the edge and its atomic can
  still read true on a press that lands on a grab. The `Window`'s `resize-border-width` stays
  unbound: Slint's handler for it gives a corner only a band-wide square, which on a rounded window
  sits in the transparent cut-out, and compares the logical band with a physical cursor, so beside
  the ring the press disagreed with the cursor. macOS answers `NotSupported` and keeps AppKit's own
  edge resizing.

- **A caption style is a chip index on the Slint side, and `settings.json` never sees it.**
  `TitlebarButtonStyle` persists by name, but every Slint property carrying it holds the index
  `window_settings::{idx_for, style_for}` maps to. The `options` order of each chip group *is* that
  index, in `window-chrome-section.slint` and in `mini-player-section.slint`, and
  `custom-titlebar.slint` and `MiniCaptions` pick their mounts by comparing against it. A new style
  is a variant, both arms of the mapping, a chip in both groups, a case at both mounts and a row in
  `window_settings::tests`' `CHIPS`, which pins the chips and the mounts to the mapping; a reordered
  array labels its chips with styles they don't select. A drawn style goes through
  `CaptionButton`'s `CaptionStyle` and adds no mount, so only a cluster sharing nothing with it,
  like the traffic lights, needs a branch of its own.

- **On Win32 a resize or move drag parks winit's loop, and with it every Slint `Timer` and
  `changed` handler**, so the whole responsive layer — the miniplayer swap, grid column counts,
  each `changed width` mirror — waits for the button to come up. `window_chrome::parked_loop` is
  what keeps them running and carries the argument. Its pump sites are the filter's
  `RedrawRequested` and `Moved` arms, and its `LoopTicks` is installed on the backend in `main.rs`.
  A new drag-reachable winit arm is where a third pump site would go.

- **The Native Title Bar toggle restarts** via `DialogKind.restart-titlebar` →
  `window_chrome::request_respawn_and_quit`; hydrate `Theme.use-native-titlebar` *before*
  `app.show()` so the window maps with the right frame. Slint applies `no-frame` live, so the frame
  is not what the restart is for; `window_chrome`'s module doc says what is.

- **Under the native title bar the miniplayer drops the frame**, through `app-window.slint`'s
  `frameless`, and that couples three trees. Win32, macOS and `KWin` all keep the window's rect
  across that change, so the client area grows into the frame it lost: the winit `Resized` arm
  measures **the step the client takes when the decoration changes**
  (`window_chrome::geometry::frame_allowance`), `WindowChrome.frame-allowance-*` holds it across the
  frameless span, `MiniPlayerSwitch` widens its exit edge by it and `restore_full_player` hands it
  back. Drop any link and a window parked just inside the threshold bounces in and out.
  **The step is measured rather than asked of the window**, argued at `frame_allowance`.
  **The desktops it doesn't bite answer that change with no reading at all**, X11 and a compositor
  drawing its own decorations both taking a dropped frame out of the window rather than handing it
  to the client, **so a step counts only while the decoration change is recent enough to be what
  moved the client**. `changed frameless` announces it over `WindowChrome.frame-changed` and
  `geometry::wire_frame_changes` stamps the moment; that link fails silently in both directions.
  Unwired, every step goes stale and the allowance sits back at zero. Unbounded, the leave's own
  resize is read as a frame, the exit edge lands above the window the restore just asked for and
  the miniplayer never leaves again. The
  window's own minimum gives the reading up while a frame stands: Win32 fixes a resize drag's
  minimum when the drag starts, framed, and a drag from the full UI otherwise can't take the
  frameless miniplayer down to its floor until the button comes up. The
  frame changes on the swap tick in **both** directions, under the crossfade. Returned at the
  decision to leave instead, it came back over a miniplayer still on screen, and Win32 stepped the
  visible window in by its invisible resize borders before the full UI appeared. **Those borders
  are the fourth link**: the client takes them over with the frame, so the `Resized` arm also
  measures them (`geometry::frame_margins`, Win32 only), `WindowChrome.frame-margin-*` holds them,
  and the shell and its overlays inset by them under the native miniplayer, leaving them
  transparent. The root's `resize-band` widens to cover them, so they still resize as the frame's
  borders did and the visible window keeps its edges across the swap.

- **A close from the miniplayer reopens the miniplayer**, and the launch is where the frame
  argument above bites hardest. `geometry::snapshot_into` persists the window as it closed plus
  the held full player as `full_player_geometry`, and `geometry::restore` seeds the hold back so
  the restore caption still has somewhere to go; `hold_full_player` keeps a hold already there,
  since a relaunched miniplayer's settled placement is the miniplayer itself. **`restore` ends in
  `adopt-restored-size()`, and removing it builds and looks right on Linux.** The window is created
  with the `no-frame` it had at `show()`, while the seed `Timer`'s `render-active` only reaches it
  a loop pass after `resumed` has built the window: framed, then dropped, and Win32 keeps the outer
  rect when the frame goes, so a native-titlebar miniplayer grew by the frame on every launch.
  **The window floor is spelled once, `MiniPlayer.window-min-*`**, read by the root's minimum and
  by `restore`'s guard against a hand-edited size. The restore caption keeps its own higher floor,
  which has to clear the miniplayer's exit edge.

- **`DialogKind.restart-backdrop` is the third of these and the one whose deadline is earlier than
  `app.show()`** — `BackdropFlags.aurora_backdrop` decides whether the two artwork tiers hold a
  `BlurSpec` at all, so `boot::ui_setup::chrome::apply_backdrop_style` raises it ahead of
  `install_views` rather than in `hydrate_ui_from_settings`. Same three-part shape as the tray's:
  `WindowChrome.restart-backdrop()` → `controls.rs::on_restart_backdrop`
  (`library::window::set_aurora_backdrop` + `request_respawn_and_quit`). Why it can't be live is
  `.claude/rules/ui-patterns.md`'s.

- **A restart can refuse, and the check has to sit on this side of the exit**, argued at
  **`window_chrome::request_respawn_and_quit`**. All four restart paths go through it: the
  titlebar toggle, the tray toggle, the backdrop-style toggle and the updater's "Restart Now".
  `respawn_target()` is the updater's recorded pre-swap path (`set_respawn_exe`) if there is one,
  else `utils::exe::current_exe()`, the marker-resolving form, so a package upgrade mid-session
  can't hand back a `<path> (deleted)` string. Don't spell the flag store + `quit_event_loop` pair
  anywhere else; both statics are private, so outside `window_chrome` you can't.

- **Transparent ARGB window for the rounded outline.** Slint's winit backend creates every window
  `with_transparent(true)`; `Window.background: Colors.transparent`; the shell, a rounded
  `Rectangle` (`clip: true`, `border-radius: Theme.window-radius`), holds everything the window
  paints, with only `ResizeRing` beside it, which paints nothing. Opaque + square when
  `is-maximized` or while an OS frame stands (`!frameless`). `window-radius` is the user's pick
  under the custom titlebar and the host's (`native-content-radius`) under the native one, so the
  native miniplayer keeps the frame's corners when it drops the frame.

- **The shell's clip is the only antialiased pass the corner may get.** The general rule is
  `slint-pitfalls.md`'s; beside the OS frame the harder arc is plain to see at the same radius.
  That is why the shell has no `background` of its own (its mantle is a square child), why the three
  overlays mount inside it (their scrims otherwise square off the corners over the desktop), and why
  the outline is an `EdgeOutline`. `app-window.slint` argues each at its mount; a new element
  reaching the window's edge goes inside the shell too.

- **The content panel inside the shell is rounded without a clip.** A rounded one held the whole
  view as a texture, rendered again on every scroll and visualizer frame, and under the custom
  titlebar nested inside the shell's own. The panel's clip has no radius and `RoundedFrame` masks
  its corners in `WindowChrome.mantle`, which is right only because the shell's ground is all that
  shows around the panel. Something painted there later needs the masks rethought, not recoloured.

- **A frameless window draws its own 1 px outline**, the edge every OS frame has, as an
  `EdgeOutline` mounted last in the shell under the same `frameless && !is-maximized` gate, so it
  outlines the whole window under the custom titlebar and only the miniplayer under the native one.
  The Settings rows stay live in both modes for that reason. `ui::appearance::window_border` paints
  `WindowChrome.border-color{,-unfocused}` from the palette republish; its System colour is the
  Windows accent while the user has it on window borders (`platform::window_border`, the registry),
  and Windows says nothing when that switch moves, so the winit `Focused` arm asks again on a
  focus gain.

- **Match Unfocused Window Background (KDE-only)** tints the full UI's chrome to the OS unfocused
  titlebar. `LayoutFlags.match_unfocused_to_system_bg`; hidden off-KDE, disabled in
  custom-titlebar. **A fresh KDE install opens on the native titlebar with the tint on, both set
  at first launch** (`WindowFlags::first_launch`, `LayoutFlags::first_launch`); the field's serde
  default `is_kde_desktop()` reaches only a file missing the key, never a fresh install.
  `Theme.window-focused` mirrors winit `Focused(bool)` raw.
  **The gate is one brush, `WindowChrome.mantle`, which argues its three terms and the missing
  `animate`**, and every surface the tint covers reads it: the sidebar, the now-playing bar, the
  shell's ground showing through the gutter beside the content panel, the panel's corner masks,
  which paint that ground over the panel, and the swap's crossfade, which has to paint that
  ground's brush. A surface spelling the ternary again, or `Theme.mantle` where the chrome shows,
  leaves a strip that stays dark on an unfocused window.

- **Always-on-top** is winit's window level on Windows, macOS and X11, and D-Bus to KWin or GNOME
  (`window-calls` ext.) on Wayland; bare GNOME falls back to native decorations.
  **Nothing done through `with_winit_window` before `app.show()` reaches the OS.** Slint creates
  the winit window only once the loop runs, the accessor answers `None` until then, and a
  `let _ =` swallows that. The restored pin went that way and painted an active icon over an
  unpinned window. Wait with `winit_window().await`, as `window_chrome::seed_always_on_top` does,
  or ride the attributes hook, as maximized does.
  **On KWin the pin is read back as well as written**, because the window manager's own keep-above
  titlebar button moves it without telling the client. `always_on_top::kwin::watch_keep_above`
  leaves a script loaded for the session that calls `Changed(b)` on Melodia's connection, and
  `window_chrome::follow_reported_pin` mirrors each report into `WindowChrome.always-on-top-active`
  and `library::window::record_always_on_top`. A report matching the button is Melodia's own
  toggle coming back and writes nothing. Three things hold it together, each argued at its
  definition: the receiver is sequential (`spawn = false`), the script tracks windows added later
  under both Plasma signal names, and it is named per data root so a relaunch replaces its
  predecessor. `kwin_tests` pins the script against the interface it calls. GNOME's `window-calls`
  and the winit arm report nothing, so there a pin moved from the titlebar still leaves the button
  behind.

- **OS file drag-and-drop rides the vendored winit fork**: stock 0.30.13 has no `wl_data_device`,
  and on no platform says where a file drag is. `winit/` is 0.30.13 + 4 commits (PR #4009, a
  `WindowId` fix, URI percent-decoding, the positioned drag events), wired by `[patch.crates-io]`.
  On Wayland, X11 and Windows it sends `DragEntered`/`DragMoved`/`DragDropped`/`DragLeft`, one per
  set of files and each carrying the pointer, in the shape winit's `0.31.0` betas shipped (PR
  #4079); macOS keeps the per-file events, which nothing reads. `winit/README.md` says what each
  commit changed. Don't bump winit without a rebase and re-sync; the root `CLAUDE.md`'s Known Gaps
  has what retires it.

- **Where a file drop lands is decided in Rust, against rects Slint publishes**, Slint having no
  pointer during an OS drag. `WindowRectReport` writes the queue panel's body and the content
  panel into `FileDrag`, the content panel's report also carrying its width into
  `WindowChrome.content-panel-width` for the grid cover tier (`ui-patterns.md`'s Covers), and
  `window_chrome::file_drop::route` hit-tests every move and the release
  against whichever can take a drop: the queue while `Queue.shown`, the content panel while
  `playlists::detail_on_screen` names a playlist and the overlay isn't up. The filter writes the
  answer to `FileDrag.target`, the one thing both banners read, so a banner always names where the
  files go, and a drop starts one import. **A report sits inside no `if` but the full-UI branch**,
  which the miniplayer swap drops on a tick of its own: it watches its own geometry, so any other
  branch is the `changed`-in-`if` panic (`slint-pitfalls.md`), which is why the playlist's target
  is the whole content panel rather than the list that unmounts inside the band's morph.

## Opening a file from the file manager

The other way paths arrive from outside, and the one that can arrive before there is a window.
`main()`'s ordering constraints are the root `CLAUDE.md`'s; this is the shape.

- **Two verbs, and they are not one.** A drop appends (`queue_import_files` when let go over the
  queue panel, routed by `file_drop`, *discarded* over anything but it and an open playlist); an open
  **replaces the queue and plays** (`queue::open_files`). They share `sort_for_queue` and nothing
  else. **Neither existing fn was the whole answer**: `queue_import_files` never sets
  `current_index`, and `player_play_tracks` wants ids in order where
  `ImportFilesResult::track_ids` is partly `HashMap` order.

- **A cold start with files skips resume-on-startup and can only log a failure**, argued at
  `main()`'s branch and at `boot::tasks::open_startup_files`.

- **A warm one raises through `tray_bridge::raise_window`**, the only thing that knows whether the
  window is hidden (tray or minimize → `show_window` + `SAVED_WINDOW_GEOM`) or merely buried (→
  winit `focus_window`, a documented no-op on Wayland). Raised either way — an empty forward is
  someone launching Melodia to get at the window.

- **The single-instance socket argues itself in `services::platform::single_instance`**, and the
  Windows half is the one no Linux runner sees: "the name is taken" has two spellings
  (`name_is_taken`, whose pure half `a_taken_name_is_recognised_in_both_spellings` asks both ways),
  and a named pipe takes no I/O deadline (`allow_missing_timeout`), either one landing a relaunch
  on `Claim::Unenforced` and a second writer over one database. Where a deadline would have bounded
  a read, reach for a thread, never a timeout. **The frame declares its length**, and
  `a_second_launch_hands_its_paths_to_the_first_and_stands_down` is why a real socket earns its
  setup: reading to EOF hangs both sides, which every codec test passes straight through.

## Tray and media keys

- **OS media controls** — MPRIS over `zbus` on Linux, souvlaki 0.8 for SMTC and macOS
  `MediaPlayer`. Bounded `mpsc` (cap 32) decouples the callback thread from `PlayerState`,
  `EventSink` from Slint. **Windows SMTC deferred** — souvlaki panics on a null
  `HWND` and no OS window exists at `AppState::init`, so `init_media_controls()` leaves Windows
  inert; `boot::ui_setup::install` posts a one-shot post-show `invoke_from_event_loop` grabbing
  the `HWND` and calling `MediaControlsHandle::attach_smtc`, a newly-attached `true` triggering a no-op
  `with_state_emit` to flush playback. Linux MPRIS / macOS MediaPlayer attach eagerly; `event_tx`
  retained Windows-only for the late rewire.

- **System tray** — `crates/melodia-platform/src/services/platform/tray/` cfg-split (Linux `ksni`, Win/mac `tray-icon`) behind
  a `mod.rs` façade (`TrayAction`, `TraySnapshot`, embedded `tray.png`, `init_tray`).
  `ui/shell/tray_bridge.rs` runs one task off a bounded `mpsc<TrayAction>`: playback reuses the
  media controls' `EventSink`, `ShowHideWindow`/`Quit` hop to the UI via `invoke_from_event_loop`,
  a `sinks.view_model` subscriber pushes tooltip + play/pause label. Linux eager; **Win/mac deferred,
  and dropped by `tray_bridge::shutdown()` before `process::exit` or the icon ghosts**. No SNI host
  → `init_tray` `None`/`false`, tray-less still usable; labels English-only.
  `TrayFlags.tray_enabled` (default on) gates `tray_bridge::install` from
  `boot::ui_setup::install`; flipping it is restart-gated through `DialogKind.restart-tray` → `WindowChrome.restart-tray()` →
  `controls.rs::on_restart_tray` (`library::window::set_tray_enabled` + `request_respawn_and_quit`,
  which may decline — above).

- **The tray icon follows the surface it sits on, not the app.** `tray.png` carries the mark's
  dark-surface pastels, so on a light taskbar or panel `tray/light_taskbar.rs` repaints it in
  `themes::BRAND_MARK_ON_LIGHT` through the raster's alpha, the pair `theme.slint`'s `brand-mark`
  paints the titlebar mark in; `themes::tests` pins the two together. Neither surface moves with
  the app theme, and neither OS announces every move, so `tray_bridge::refresh_icon` rides the
  winit focus-gain arm on both platforms beside each one's own change event. A switch that event
  misses, made while the window is hidden in the tray, lands at the next focus gain.
  - **Windows** reads the *Windows* mode (`color_mode::taskbar_theme`, `SystemUsesLightTheme`),
    which moves independently of the app mode winit's `ThemeChanged` tracks; that arm refreshes
    too.
  - **Linux reads a Plasma panel and nothing else** (`system_theme::plasma_panel_is_light`, gated on
    the session by `tray_bridge::on_light_panel`; each argues its half), so other desktops keep
    the asset's colours. The portal consumer in `appearance/system_watcher.rs` refreshes on a
    scheme flip, live even while hidden. A Plasma style switched on its own leaves the colour
    scheme, and so the portal, untouched.
  - **The Linux tray belongs to its subscriber task**, not the UI thread, so there `refresh_icon`
    only bumps `PANEL_RECHECK` and the task re-reads the panel under `block_in_place`.

- **Close-to-tray** (`TrayFlags.close_to_tray`, default off) — Slint `Window::hide/show` on
  `should_hide_to_tray()`, gated on the setting **and** a live tray (`SettingRow.disabled` when
  `Settings.tray-active` false), and off while an MSI update runs (argued at
  `updater::install::INSTALLER_RUNNING`). Hide snapshots
  `SAVED_WINDOW_GEOM`; re-show re-asserts it from a self-rescheduling 16 ms timer
  (`reschedule_geometry_restore`, cap `RESTORE_ATTEMPTS`).
  `WINDOW_VISIBLE` shadows visibility — `is_visible()` is `None` on Wayland.

## Shutdown

- **Force-exit.** `main()` ends in `std::process::exit(0)`, so whatever a destructor would have
  done is an explicit call ahead of it, each argued at `main()`'s tail and in `shutdown.rs`.
  `main_order_tests` pins the output close and the log flush ahead of both ways out.
