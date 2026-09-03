---
paths:
  - src/ui/window_chrome/**/*.rs
  - src/ui/shell/tray_bridge.rs
  - src/ui/shell/event_sink.rs
  - crates/melodia-platform/src/services/platform/tray/**/*.rs
  - src/services/integrations/media_controls/**/*.rs
  - crates/melodia-platform/src/services/platform/always_on_top/**/*.rs
  - crates/melodia-platform/src/services/platform/dwm_titlebar.rs
  - src/ui/appearance/theme_apply.rs
  - src/main.rs
  - src/shutdown.rs
  - src/boot/**/*.rs
  - melodia-ui/ui/app-window.slint
  - melodia-ui/ui/components/custom-titlebar.slint
  - melodia-ui/ui/components/macos-titlebar-cluster.slint
  - melodia-ui/ui/components/macos-traffic-light.slint
  - melodia-ui/ui/views/settings/window-chrome-section.slint
---

# The desktop shell — window chrome, tray, media keys

The app's relationship with the OS window server and the desktop's notification/media surfaces.
The theme running through it: **Slint's window API is a cache, winit is the truth**, and anything
the OS owns has to be attached late or not at all on at least one platform.

## Window chrome

- **Window-control APIs go through winit, not Slint.** Slint's `set_minimized`/`set_maximized`
  cache stalls on Wayland: `WinitWindowAccessor::with_winit_window(|w| w.set_minimized(true))`.

- **Window dragging belongs at the winit layer.** `drag_window()` from Slint `pointer-event(down)`
  leaks the input grab. `TouchArea` reports `has-hover` via
  `WindowChrome.drag-region-hover-changed`; `on_winit_window_event` intercepts
  `MouseInput { Pressed, Left }` when that atomic is true → `drag_window()` → `PreventDefault`.

- **On Win32 a resize or move drag parks winit's loop, and with it every Slint `Timer` and
  `changed` handler**, so the whole responsive layer — the miniplayer swap, grid column counts,
  each `changed width` mirror — waits for the button to come up. `winit_filter::pump_parked_loop`
  is what keeps them running and carries the argument; a new drag-reachable winit arm is where a
  third pump site would go.

- **`Window.no-frame` is sticky** — read once at first show. The Native Title Bar toggle restarts
  via `Dialog` `"restart-titlebar"` → `window_chrome::request_respawn_and_quit`; hydrate
  `Theme.use-native-titlebar` *before* `app.run()`.

- **`"restart-backdrop"` is the third of these and the one whose deadline is earlier than
  `app.run()`** — `BackdropFlags.aurora_backdrop` decides whether the two artwork tiers hold a
  `BlurSpec` at all, so `boot::ui_setup::apply_backdrop_style` raises it ahead of `install_views`
  rather than in `hydrate_ui_from_settings`. Same three-part shape as the tray's:
  `WindowChrome.restart-backdrop()` → `controls.rs::on_restart_backdrop`
  (`library::window::set_aurora_backdrop` + `request_respawn_and_quit`). Why it can't be live is
  `.claude/rules/ui-patterns.md`'s.

- **A restart can refuse, and the check has to sit on this side of the exit.** All four restart
  paths — titlebar toggle, tray toggle, backdrop-style toggle, the updater's "Restart Now" — go
  through
  **`window_chrome::request_respawn_and_quit`**, which resolves `respawn_target()` *before*
  setting `RESPAWN_AFTER_EXIT` and quitting: past `slint::quit_event_loop()` the window is gone, so
  a failed `exec` in `shutdown::respawn_if_requested` has nothing to fall back to and the app
  simply vanishes. With no binary to come back to it stays up and raises a sticky
  `ToastKind::RestartRequired`; every caller has persisted its setting by then, so a toast asking
  the user to close and reopen is an honest answer — the change lands on the next manual launch.
  `respawn_target()` is the updater's recorded pre-swap path (`set_respawn_exe`) if there is one,
  else `utils::exe::current_exe()` — the marker-resolving form, so a package upgrade mid-session
  can't hand back a `<path> (deleted)` string. Don't inline the flag store + `quit_event_loop` pair
  at a fourth site; outside `window_chrome` you can't, both statics being private, so the rule only
  binds *inside* that module.

- **Transparent ARGB window for the rounded outline.** winit `with_transparent(true)`;
  `Window.background: Colors.transparent`; rounded mantle `Rectangle` (`clip: true`,
  `border-radius: Theme.shell-radius`) the only direct child. Opaque + square when `is-maximized`
  or `use-native-titlebar`.

- **Match Unfocused Window Background (KDE-only)** — tints sidebar + NP-bar to the OS unfocused
  titlebar. `LayoutFlags.match_unfocused_to_system_bg`, serde default `is_kde_desktop()`; hidden
  off-KDE, disabled in custom-titlebar. `Theme.window-focused` mirrors winit `Focused(bool)` raw;
  sites gate on all three:
  `(Settings.match-unfocused-bg && Theme.use-native-titlebar && !Theme.window-focused)`
  `? mantle-unfocused : mantle`. No `animate` — desyncs the OS swap.

- **Always-on-top (Linux)** — D-Bus to KWin or GNOME (`window-calls` ext.); bare GNOME falls back
  to native decorations.

- **OS file drag-and-drop rides the vendored winit fork** — stock 0.30.13 has no `wl_data_device`.
  `winit/` is 0.30.13 + 3 commits (PR #4009, `WindowId` fix, URI percent-decoding, cfg-gated to
  Linux), wired by `[patch.crates-io]`. Flow: `winit_filter.rs::DroppedFile` →
  `drop_coalescer.rs::schedule_drop_flush` (50 ms → `queue_import_files`);
  `HoveredFile{,Cancelled}` toggle `Queue.is-drop-hovered`. Don't bump winit without a rebase +
  re-sync — the root `CLAUDE.md`'s Known Gaps has what retires it.

## Opening a file from the file manager

The other way paths arrive from outside, and the one that can arrive before there is a window.
`main()`'s ordering constraints are the root `CLAUDE.md`'s; this is the shape.

- **Two verbs, and they are not one.** A drop appends (`queue_import_files`, routed by
  `drop_coalescer`, *discarded* when neither the queue sheet nor a playlist is open); an open
  **replaces the queue and plays** (`queue::open_files`), as VLC, foobar2000 and Strawberry all
  default to. They share `sort_for_queue` and nothing else. **Neither existing fn was the whole
  answer**: `queue_import_files` never sets `current_index`, and `player_play_tracks` wants ids in
  order where `ImportFilesResult::track_ids` is partly `HashMap` order.

- **A cold start with files skips resume-on-startup** — `open_startup_files` runs synchronously
  after `restore_persisted_playback`, so resume would only be visible for the moment it takes them
  to land. It is also **too early to toast**: that bridge installs with the UI and drops rather than
  queues, so a cold-start failure can only log.

- **A warm one raises through `tray_bridge::raise_window`**, the only thing that knows whether the
  window is hidden (tray or minimize → `show_window` + `SAVED_WINDOW_GEOM`) or merely buried (→
  winit `focus_window`, a documented no-op on Wayland). Raised either way — an empty forward is
  someone launching Melodia to get at the window.

- **"The name is taken" has two spellings and `interprocess` normalises neither.** Unix `bind`
  says `EADDRINUSE`; a Windows named pipe is created under `FILE_FLAG_FIRST_PIPE_INSTANCE`, whose
  second instance fails `ERROR_ACCESS_DENIED` — `PermissionDenied`, nowhere near `AddrInUse`.
  Matching only the first left *every* Windows relaunch `Claim::Unenforced`: a second window and a
  second writer over one database, on the platform the MSI registers associations for.
  `name_is_taken` is the single place that decides, and **its pure half takes the platform as an
  argument** — `utils::redact::redact_prefix`'s shape, for the same reason. A `cfg!` inside the
  predicate is a branch the Linux gate compiles out and can never exercise, so a
  "simplification" back to one spelling merges green; `name_is_taken_on` is what
  `a_taken_name_is_recognised_in_both_spellings` can ask both ways. A genuine ACL denial takes the
  same arm and fails at the connect, landing back on `Unenforced`, which is where it belonged
  anyway.
  **Recognising the name is only half of it — the forward has to survive the transport too.** A
  named pipe has no settable I/O timeout, so `forward`'s deadline came back `Unsupported` and
  propagated, landing on the same `Unenforced` arm and the same second window; `allow_missing_timeout`
  carries that argument and is called at all three deadline sites. **What the deadline was *for*
  doesn't go away with it, and neither site may hold a blocking read on a thread something else
  needs**: `spawn_reader` takes each connection off the accept loop under a cap, `wait_for_close`
  bounds the forwarder's ack the same way. Reach for a thread there, never a timeout — one
  transport won't take one.

- **The accept loop is a detached `std::thread`, not `spawn_blocking`**, as `discord/ipc.rs` runs
  its transport: a parked blocking-pool tenant is what the 32-slot cap exists to prevent. Its
  read-failure arm still calls `on_launch` with no paths: the connection is proof of a launch, and
  a selection past `MAX_PAYLOAD_LEN` should cost the user their file list, not their window.

- **The frame declares its length, and that is not decoration.** Reading to EOF instead means the
  receiver waits on the sender's close while the sender waits to know the payload landed — **both
  block to their 2 s timeouts and the paths are dropped**, which every codec test passes straight
  through. `a_second_launch_hands_its_paths_to_the_first_and_stands_down` is why a real socket
  earns its setup. The sender *does* wait for the close, deliberately: exiting straight after the
  write leaves the payload in a pipe buffer with its own handle the last one open, on the platform
  no Linux runner covers.

## Tray and media keys

- **OS media controls** — souvlaki 0.8. Bounded `mpsc` (cap 32) decouples the callback thread from
  `PlayerState`, `EventSink` from Slint. **Windows SMTC deferred** — souvlaki panics on a null
  `HWND` and no OS window exists at `AppState::init`, so `init_media_controls()` leaves Windows
  inert; `main()` posts a one-shot post-show `invoke_from_event_loop` grabbing the `HWND` and
  calling `MediaControlsHandle::attach_smtc`, a newly-attached `true` triggering a no-op
  `with_state_emit` to flush playback. Linux MPRIS / macOS MediaPlayer attach eagerly; `event_tx`
  retained Windows-only for the late rewire.

- **System tray** — `src/services/platform/tray/` cfg-split (Linux `ksni`, Win/mac `tray-icon 0.24`) behind
  a `mod.rs` façade (`TrayAction`, `TraySnapshot`, embedded `tray.png`, `init_tray`).
  `ui/shell/tray_bridge.rs` runs one task off a bounded `mpsc<TrayAction>`: playback reuses
  souvlaki's `EventSink`, `ShowHideWindow`/`Quit` hop to the UI via `invoke_from_event_loop`, a
  `sinks.view_model` subscriber pushes tooltip + play/pause label. Linux eager; **Win/mac deferred,
  and dropped by `tray_bridge::shutdown()` before `process::exit` or the icon ghosts**. No SNI host
  → `init_tray` `None`/`false`, tray-less still usable; labels English-only. **Opt-in**
  `TrayFlags.enabled` (default off) gates `tray_bridge::install` from `main.rs`; flipping it is
  restart-gated through `restart-tray` `Dialog` → `WindowChrome.restart-tray()` →
  `controls.rs::on_restart_tray` (`library::window::set_tray_enabled` + `request_respawn_and_quit`,
  which may decline — above).

- **Close-to-tray** (`TrayFlags.close_to_tray`, default off) — Slint `Window::hide/show` on
  `should_hide_to_tray()`, gated on the setting **and** a live tray (`SettingRow.disabled` when
  `Settings.tray-active` false). Hide snapshots `SAVED_WINDOW_GEOM`; re-show re-asserts it from a
  self-rescheduling 16 ms timer (`reschedule_geometry_restore`, cap `RESTORE_ATTEMPTS`).
  `WINDOW_VISIBLE` shadows visibility — `is_visible()` is `None` on Wayland.

## Shutdown

- **Force-exit.** `main()` ends in `std::process::exit(0)`; a normal return lets souvlaki's MPRIS
  thread, accesskit's a11y thread and any tokio worker parked on a blocking call pin the process.
  The audio output used to be a fourth and no longer is — `AudioOutput` is owned on `AppState` since
  #90 — which changes nothing here. `tracker.wait()` + `db.close()` in a 3 s
  `timeout`, runtime dropped on a background thread; `save_state_on_exit` flushes synchronously
  *before* the timeout.
