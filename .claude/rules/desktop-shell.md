---
paths:
  - crates/melodia-views/src/ui/window_chrome/**/*.rs
  - crates/melodia-views/src/ui/shell/tray_bridge.rs
  - crates/melodia-views/src/ui/appearance/theme_paint.rs
  - crates/melodia-views/src/ui/appearance/window_border.rs
  - crates/melodia-views/src/ui/appearance/window_settings.rs
  - crates/melodia-views/src/ui/callbacks/updater/mod.rs
  - crates/melodia-platform/src/services/platform/tray/**/*.rs
  - crates/melodia-platform/src/services/platform/registry.rs
  - crates/melodia-integrations/src/services/integrations/media_controls/**/*.rs
  - crates/melodia-app/src/services/settings/interface.rs
  - crates/melodia-app/src/tasks/device_volume.rs
  - crates/melodia/src/main.rs
  - crates/melodia/src/boot/**/*.rs
  - crates/melodia-ui/ui/app-window.slint
  - crates/melodia-ui/ui/globals/dialog.slint
  - crates/melodia-ui/ui/globals/shell.slint
  - crates/melodia-ui/ui/layout/sidebar.slint
  - crates/melodia-ui/ui/layout/now-playing-bar.slint
  - crates/melodia-ui/ui/views/queue-panel/queue-panel.slint
  - crates/melodia-ui/ui/components/custom-titlebar.slint
  - crates/melodia-ui/ui/components/caption-buttons.slint
  - crates/melodia-ui/ui/components/mini-player/*.slint
  - crates/melodia-ui/ui/views/settings/window-chrome-section.slint
  - crates/melodia-ui/ui/views/settings/mini-player-section.slint
---

# The desktop shell: window chrome, tray, media keys

**Slint's window API is a cache and winit is the truth**, and what the OS owns attaches late, or
not at all, on at least one platform.

## Window chrome

- **Window controls go through winit, never `slint::Window::set_minimized`/`set_maximized`**,
  which forward only when a cached flag differs, a flag Wayland never clears
  (`window_chrome::controls`).
- **Nothing done through `with_winit_window` reaches the OS before the event loop builds the
  window**, the accessor answering `None`, which a `let _ =` swallows. Await `winit_window()`, as
  `window_chrome::seed_always_on_top` does, or ride the attributes hook, as maximized does.
- **A caption style is a chip index in Slint and a name in `settings.json`**
  (`window_settings::{idx_for, style_for}`), each chip group's `options` order being the index.
  `idx_for` is exhaustive, but nothing fails on a new style missing `style_for`'s arm, its chip in
  `window-chrome-section.slint` and `mini-player-section.slint`, or its case in
  `custom-titlebar.slint` and `MiniCaptions`. A drawn style rides `CaptionStyle` and adds no branch.
- **The native titlebar, the tray icon and the backdrop style restart rather than apply**, boot
  being each one's only reader: a `DialogKind.restart-*`, a `WindowChrome` callback and
  `window_chrome::controls`' `restart_toggle`.
- **Every respawn goes through `window_chrome::request_respawn_and_quit`**, which can refuse and
  argues why the check sits before the exit.
- **What the OS changes unannounced is re-read in the winit `Focused` arm**: the Windows app mode,
  the accent a window border takes, the tray's surface. A new OS-owned colour joins that arm.
- **The unfocused tint is one brush, `WindowChrome.mantle`**, which argues its terms, and every
  surface the chrome shows through reads it. One spelling the ternary, or `Theme.mantle`, stays
  dark on an unfocused KDE window.

## Tray and media keys

- **Hiding and showing go through `tray_bridge::hide_window` and `raise_window`**, the only paths
  that know `SAVED_WINDOW_GEOM`, and every visibility change through `set_window_visible`, whose
  shadow stands in for winit's `is_visible()`, `None` on Wayland.
- **`player::engine::state::volume_to_amplitude` is the one volume conversion**, which the OS media
  panel and the device's own control both take; never re-roll `/ 100`.
