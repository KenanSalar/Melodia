---
paths:
  - crates/melodia-views/src/ui/radio/**/*.rs
  - crates/melodia-app/src/library/radio/**/*.rs
  - crates/melodia-app/src/library/radio_files.rs
  - crates/melodia-net/src/services/net/radio_browser/**/*.rs
  - crates/melodia-audio/src/player/source/stream_source.rs
  - crates/melodia-audio/src/player/source/prebuffer.rs
  - crates/melodia-audio/src/player/source/stream_decode.rs
  - crates/melodia-net/src/media/fetch/station_logo.rs
  - crates/melodia-artwork/src/media/image/logo_tile.rs
  - crates/melodia-core/src/entities/radio.rs
  - crates/melodia-store/src/database/queries/radio.rs
  - crates/melodia-app/src/tasks/radio_logo_cache.rs
  - crates/melodia-app/src/library/settings/radio.rs
  - crates/melodia-views/src/ui/settings/radio_settings.rs
  - crates/melodia-ui/ui/globals/radio.slint
  - crates/melodia-ui/ui/components/station-facts.slint
  - crates/melodia-ui/ui/views/radio-view.slint
  - crates/melodia-ui/ui/views/radio/browse-tab.slint
  - crates/melodia-ui/ui/views/radio/kept-stations-tab.slint
  - crates/melodia-ui/ui/views/radio/station-detail.slint
  - crates/melodia-ui/ui/views/radio/station-card.slint
  - crates/melodia-ui/ui/views/radio/station-grid.slint
  - crates/melodia-ui/ui/components/grid/station-context-menu.slint
  - crates/melodia-ui/ui/views/radio/facet-chip.slint
  - crates/melodia-ui/ui/views/radio/suggestion-pill.slint
  - crates/melodia-ui/ui/views/radio/tab-pills.slint
  - crates/melodia-ui/ui/views/settings/radio-section.slint
  - crates/melodia-ui/ui/components/dialog/add-station-body.slint
  - migrations/20260820000000_radio_stations.sql
---

# Internet radio

What no single file holds: an off switch enforced in one place and reachable from many, two tables
the artwork sweep has to know, a stream whose prohibitions reach out of `melodia-audio`, and the
numbers spelled in both trees. The page's shape is `ui-patterns.md`'s, the sweep and its ledger
`library-data.md`'s, and the ring and the feed thread's reconnect `audio-stack.md`'s live source.

## The off switch

- **`library::radio` is the one door, and the door is the directory, not `mod.rs`** (its `//!`,
  ADR 15). `crates/melodia/tests/radio_facade.rs` holds both sides, reading the facade off its
  directory so a new submodule is covered unedited:
  - `http_client()` and `radio_enabled` are each named once in the facade, inside
    `directory_client` and `ensure_enabled`, and the facade builds no client of its own;
  - `play_station` asks `ensure_enabled` itself, its stream coming off `PlaybackContext.http`;
  - nothing outside the facade names `radio_browser`, and nothing but the facade and
    `library::radio_files` names `queries::radio`, where a station write elsewhere in
    `melodia-app` reads as ordinary transport code.
- **`ui::radio::disable` is everything off undoes past the row and the router branch**, the nav
  history included: a live index left on 10 paints an empty panel, the placeholder fall-through
  excluding 10. A new way onto nav 10 owes it a line; `fold_disabled_nav_index` is the one fold,
  at boot and live.
- **The `SectionActiveGate` at index 10 carries no `if`.** Its `changed` tracker reads `Nav`,
  which outlives any branch, so a dropped one panics on the next nav change (`slint-pitfalls.md`).
  With the row gone the index is unreachable: gate the row and the router branch, never the mount.

## Stations on disk

- **Two tables, and only `radio_stations` holds what the user owns**: favorites, hand-typed URLs
  and play history, as one row. `radio_logo_answers` records our own fetch outcomes by URL, which
  is why it may exist when browsed stations are never persisted. Both `artwork_path`s are in
  `queries::artwork::ARTWORK_COLUMNS` (`library-data.md`) and neither file can be re-derived.
  `tasks::radio_logo_cache`'s `//!` argues why its prune runs before the sweep.
- **Each of the two migrations argues its own columns in its header**, the four `local_*` columns
  that give each field one writer among them. Both have shipped, so a schema change is a third.

## The stream

- **Nothing logs a stream URL**, which can carry a session token. `PlayerAction`'s `Display` prints
  the generation and `stream_source` names the station; `RadioStationRow` has no `stream_url`, the
  URL crossing only on `RadioVm` and `Radio.detail-stream-url`.
- **Nothing calls `StreamDownload`'s convenience constructors**, which build an unconfigured
  client of their own: open through `HttpStream::new` with the shared one
  (`crates/melodia/tests/stream_client.rs`). `stream-download`'s `default-features = false` is
  load-bearing, argued at the root `Cargo.toml`.
- **A station is not a queue entry**: starting one stops the queue and leaves it seated underneath,
  which is what hands the library back on stop, and `queue.json` holds the station's row id beside
  it.

## Cards and history

- **A station card selects by `RadioStationRow.select_key`, never `id`**, one `CardSelection` scope
  per tab: the row id on the kept tabs, the place in the answer on Browse, whose rows all hold
  `id == 0`. A fresh answer renumbers, so `browse::paint` drops a Browse set, and every batch
  action resolves through its tab's `resolve_keys`.
- **A history row splits through the engine's `Announcement::parse`**, the call behind the bar and
  the integrations, so Copy Title and Copy Artist name what the bar showed.

## Numbers both trees spell

Each is written once in Rust and again in `.slint`, so each has a pin or an argument.

- **`logo_tile::MIN_LOGO_DIM` is restated in `now-playing/source-artwork.slint`**, which binds no
  `native-size` on the strength of it; `logo_tile_tests.rs` pins the two together.
- **`Radio.tab-count` is the sole tab count**: `seed_tab` clamps against it and `tab_from_index`
  falls to Browse, both pinned in `radio_tests.rs`.
- **`NAV_RADIO` and `services::view_state::MAX_NAV_INDEX` are both 10**, the second bounding the
  persisted nav index at both ends of its round trip.
- **`id == 0` means "no database row"**, given that meaning once in `rows::station_has_row` and
  spelled by hand where `melodia-views` can't be named: `record_probed_codec` and
  `PlayerState::to_persisted`. Grep the raw spelling when its meaning moves.
- **The `-1` sentinels are declared twice by hand**: `NO_SEAT`, `VOTES_UNKNOWN` and
  `tab_bar::UNFETCHED_COUNT`, against their `globals/radio.slint` defaults. The facet chip indices
  are safe, `facets::chip_indices` reading them off the global.
