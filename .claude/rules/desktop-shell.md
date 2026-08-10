---
paths:
  - src/ui/window_chrome/**/*.rs
  - src/ui/shell/tray_bridge.rs
  - src/ui/shell/event_sink.rs
  - src/services/tray/**/*.rs
  - src/services/media_controls/**/*.rs
  - src/services/always_on_top/**/*.rs
  - src/services/dwm_titlebar.rs
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

Everything about the app's relationship with the OS window server and the desktop's
notification/media surfaces. The theme running through it: **Slint's window API is a
cache, winit is the truth**, and anything the OS owns has to be attached late or not
at all on at least one platform.

## Window chrome

- **Window-control APIs go through winit, not Slint.** Slint's `set_minimized`/`set_maximized` cache stalls on Wayland. Use `WinitWindowAccessor::with_winit_window(|w| w.set_minimized(true))`.
- **Window dragging belongs at winit layer.** `drag_window()` from Slint `pointer-event(down)` leaks input grab. `TouchArea` reports `has-hover` via `WindowChrome.drag-region-hover-changed`; `on_winit_window_event` intercepts `MouseInput { Pressed, Left }` when atomic true → `drag_window()` → `PreventDefault`.
- **`Window.no-frame` is sticky.** Slint reads once at first show. Native Title Bar toggle requires restart via `Dialog` `"restart-titlebar"` → `RESPAWN_AFTER_EXIT`. Hydrate `Theme.use-native-titlebar` *before* `app.run()`.
- **Transparent ARGB Window for rounded outlines.** winit `with_transparent(true)`; `Window.background: Colors.transparent`; rounded mantle Rectangle (`clip: true`, `border-radius: Theme.shell-radius`) is the only direct child. Opaque + square when `is-maximized` or `use-native-titlebar`.
- **Match Unfocused Window Background (KDE-only).** Tints sidebar + NP-bar to OS unfocused titlebar. `LayoutFlags.match_unfocused_to_system_bg`, serde default `is_kde_desktop()`; hidden off-KDE, disabled in custom-titlebar. `Theme.window-focused` mirrors winit `Focused(bool)` raw. Sites gate on all three: `(Settings.match-unfocused-bg && Theme.use-native-titlebar && !Theme.window-focused) ? mantle-unfocused : mantle`. No `animate` — desyncs OS swap.
- **Always-on-top (Linux)** — D-Bus to KWin or GNOME (`window-calls` ext.); falls back to native decorations on bare GNOME.
- **OS file drag-and-drop via vendored winit fork.** Stock winit 0.30.13 has no `wl_data_device`. Fork at `winit/` (0.30.13 + 3 commits: PR #4009 + `WindowId` fix + URI percent-decoding, cfg-gated to Linux), wired via `[patch.crates-io] winit = { path = "winit" }`. Flow: `winit_filter.rs::DroppedFile` → `drop_coalescer.rs::schedule_drop_flush` (50 ms → `queue_import_files`); `HoveredFile{,Cancelled}` toggle `Queue.is-drop-hovered`. Don't bump winit without rebase + re-sync — see the Known Gaps entry in the root `CLAUDE.md` for what retires it.

## Tray and media keys

- **OS media controls** — souvlaki 0.8. Bounded `mpsc` (cap 32) decouples callback thread from `PlayerState`; `EventSink` trait decouples from Slint. **Windows SMTC deferred** — souvlaki panics on null `HWND` and no OS window exists at `AppState::init`, so `init_media_controls()` leaves Windows inert. `main()` posts a one-shot `invoke_from_event_loop` post-show that grabs `HWND` + calls `MediaControlsHandle::attach_smtc`; newly-attached returns `true`, triggering a no-op `with_state_emit` to flush playback. Linux MPRIS / macOS MediaPlayer attach eagerly. `event_tx` retained Windows-only for late rewire.
- **System tray** — `src/services/tray/` cfg-split: Linux `ksni`; Win/mac `tray-icon 0.24`. Façade `mod.rs` (`TrayAction`, `TraySnapshot`, embedded `tray.png`, `init_tray`). `src/ui/shell/tray_bridge.rs`: bounded `mpsc<TrayAction>` → one task — playback reuses souvlaki `EventSink`; `ShowHideWindow`/`Quit` hop UI via `invoke_from_event_loop`; `sinks.view_model` subscriber pushes tooltip + play/pause label. Linux eager; **Win/mac deferred**, dropped by `tray_bridge::shutdown()` pre-`process::exit` or it ghosts. No SNI host → `init_tray` `None`/`false`; tray-less still usable. **Opt-in** `TrayFlags.enabled` (default off): `main.rs` gates `tray_bridge::install`. Restart-gated via `restart-tray` `Dialog` → `WindowChrome.restart-tray()` → `controls.rs::on_restart_tray` (`library::window::set_tray_enabled` + `RESPAWN_AFTER_EXIT`). **Close-to-tray** `TrayFlags.close_to_tray` (default off): Slint `Window::hide/show` on `should_hide_to_tray()` — gated on setting AND active tray; `SettingRow.disabled` when `Settings.tray-active` false. Hide→show snapshots into `SAVED_WINDOW_GEOM`; re-show re-asserts via self-rescheduling 16 ms timer (`reschedule_geometry_restore`, cap `RESTORE_ATTEMPTS`). Tray labels English-only. `WINDOW_VISIBLE` atomic shadows visibility (`is_visible()` is `None` on Wayland).

## Shutdown

- **Force-exit shutdown.** `main()` ends `std::process::exit(0)`; normal return lets leaked `MixerDeviceSink`/MPRIS/accesskit pin the process. `tracker.wait()` + `db.close()` wrapped in a 3 s `timeout`; runtime dropped on a background thread. `save_state_on_exit` flushes synchronously *before* the timeout.
