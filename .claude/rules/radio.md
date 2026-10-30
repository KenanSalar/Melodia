---
paths:
  - crates/melodia-views/src/ui/radio/**/*.rs
  - crates/melodia-views/src/ui/nav_history.rs
  - crates/melodia/src/boot/ui_setup/views.rs
  - crates/melodia-ui/ui/app-window.slint
  - crates/melodia-ui/ui/layout/sidebar.slint
  - crates/melodia-app/src/library/radio/**/*.rs
  - crates/melodia-app/src/library/playback.rs
  - crates/melodia-audio/src/player/source/stream_source.rs
  - crates/melodia-engine/src/player/engine/**/*.rs
---

# Internet radio

- **A new way onto nav 10 owes `ui::radio::disable` a line**, or switching Radio off leaves a route
  onto an empty panel.
- **Nothing logs a stream URL**, which can carry a session token without reading as one.
- **`id == 0` means "no database row"**, given that meaning once in `rows::station_has_row` and
  spelled by hand where `melodia-views` can't be named: `record_probed_codec` and
  `PlayerState::to_persisted`. Grep the raw spelling when its meaning moves.
