---
paths:
  - src/ui/radio/**/*.rs
  - src/library/radio.rs
  - src/library/radio_files.rs
  - src/services/radio_browser/**/*.rs
  - src/player/stream_source.rs
  - src/player/prebuffer.rs
  - src/player/stream_decode.rs
  - src/media/station_logo.rs
  - src/entities/radio.rs
  - src/database/queries/radio.rs
  - src/tasks/radio_logo_cache.rs
  - src/library/settings/radio.rs
  - src/ui/settings/radio_settings.rs
  - melodia-ui/ui/globals/radio.slint
  - melodia-ui/ui/components/station-facts.slint
  - melodia-ui/ui/views/radio-view.slint
  - melodia-ui/ui/views/radio/browse-tab.slint
  - melodia-ui/ui/views/radio/kept-stations-tab.slint
  - melodia-ui/ui/views/radio/station-detail.slint
  - melodia-ui/ui/views/radio/station-card.slint
  - melodia-ui/ui/views/radio/station-grid.slint
  - melodia-ui/ui/views/radio/facet-chip.slint
  - melodia-ui/ui/views/radio/suggestion-pill.slint
  - melodia-ui/ui/views/radio/tab-pills.slint
  - melodia-ui/ui/views/settings/radio-section.slint
  - melodia-ui/ui/components/dialog/add-station-body.slint
  - migrations/20260820000000_radio_stations.sql
---

# Internet radio

The couplings no single file can hold: an off switch enforced one place and observable from four,
two tables the artwork sweep has to know about, a stream whose prohibitions and dependency pins
reach out of `src/player/`, and a handful of numbers spelled in both trees.

The page's own shape belongs to `.claude/rules/ui-patterns.md` — the Browse box and its scope
pills, `StationCard` as an `EntityCard` host, and why three tabs share one `SectionActiveGate`.
The sweep is `library-data.md`'s. The stream's own design, the ring included, is
`src/player/CLAUDE.md`'s. None of those is restated here, and a paragraph here that starts to
is the copy to delete.

## The off switch

- **The switch itself is argued on `library::radio`'s own items** — why it sits at the facade,
  what the getters are exempt from, and why the logo download is not. What spans is the pair of
  walks holding it, one per direction and neither covering the other's half:
  `library::radio::tests::every_outbound_call_takes_its_client_from_behind_the_switch` counts the
  reaches inside the facade, `radio_browser::tests::only_the_radio_facade_reaches_the_directory_client`
  counts them outside it. So **`http_client()` may be named exactly once in `library/radio.rs`**
  and `radio_browser` may be named nowhere else in the tree.
- **`station_to_restore` is the one getter guarded like a play**, what it does being to put a
  station back on the deck rather than to answer a question about one.
- **Turning it off has four consequences beyond the row, and three are findable only from here.**
  `ui::radio::disable` stops a playing station, sweeps section 10 out of the nav history, closes and
  forgets every seated detail, and folds a live `Nav.selected-index` off 10. The history sweep is
  the one the plan missed: the row and the router branch both go, but a Mouse-4 walk would still
  land on nav 10, and the `PlaceholderView` fall-through keeps a plain `!= 10` term — so the panel
  behind it paints *nothing at all*, not a placeholder.
- **`fold_disabled_nav_index` is a sibling of `my_library::fold_retired_nav_index`, never an arm
  inside it.** They answer different questions: one index was retired, which is true of a file
  forever, where the other is unreachable *in this install* and flips with a setting.
  `boot::ui_setup` composes them. The live move goes through the same function as the boot fold, so
  the two cannot disagree about where 10 goes.
- **The `SectionActiveGate` mount at index 10 carries no `if`.** It holds a `changed` tracker and a
  dropped tracker-bearing branch panics; with the sidebar row gone the index is simply unreachable,
  so the gate never fires true. Gate the row and the router branch, not the mount.

## Stations on disk

- **Two tables, and only one holds rows the user owns.** `radio_stations` is favorites, hand-typed
  URLs and play history at three points in one row's life. `radio_logo_answers` is a record of our
  own network outcomes keyed on the URL — which is what lets it exist beside the rule that browsed
  stations are never persisted (D3): it describes what a fetch returned, not what the directory
  said.
- **Both carry an `artwork_path` and both are in the sweep's ledger.** Six columns now, in both
  halves of `queries::artwork`'s pair. Miss either and station logos are deleted on the next sweep,
  with no way to re-derive them.
- **The prune-then-sweep order, and why its trigger is a Radio leave rather than a scan, are
  `tasks::radio_logo_cache`'s own `//!`.** The ledger both halves turn on is `library-data.md`'s.
- **Every column has exactly one writer, which is what the four `local_*` columns buy.** The
  directory rewrites `homepage`, `tags`, `country` and `favicon_url` wholesale on the next
  re-import, so a user edit folded into them would be silently reverted; kept apart, a reader takes
  the local one first and both writers stay correct. `RadioStation::can_override` is why a field is
  offered only where the directory said nothing.
- **The migration is branch-local until this ships, so fold changes into it** rather than adding a
  second. Its header argues the schema — no `AUTOINCREMENT`, no secondary indexes, why `hls` and
  `country` are stored rather than re-derived — and none of that is restated here.
- **A deleted station's id can be reused**, there being no `AUTOINCREMENT`. So a persisted
  `last_detail_ids` entry can land on a different station across a restart, exactly as it can for a
  playlist. That is the accepted cost of not upserting every station the user merely glanced at.

## The stream

- **D8, the reconnect's home and what the ring is still for are `src/player/CLAUDE.md`'s**,
  over `prebuffer.rs`'s and `stream_decode.rs`'s `//!`. That file loads on the whole directory,
  including `handlers.rs` — the monitor the reconnect argument is *about*, and the one file in it
  no glob here reaches.
- **The feed thread's name is spelled inline and may not become a const.**
  `services::tests::no_thread_name_outgrows_what_the_kernel_keeps` matches a literal after
  `.name(`, so lifting `"radio-buffer"` out would leave it silently unmeasured — the one refactor
  here that looks like an improvement and disables a check.
- **Nothing may call `StreamDownload::new_http`** — it constructs its own unconfigured client behind
  your back, losing both the shared pool and the `Icy-MetaData` header. Go through `HttpStream::new`
  with the shared-client newtype; held by a corpus walk, since the temptation is a one-line
  constructor.
- **The two dependencies are feature-pinned and the pin is load-bearing.** `stream-download`'s
  defaults pull `reqwest/default-tls`, which is OpenSSL on Linux and changes what `cargo-deb`'s
  `$auto` resolves to. `default-features = false` plus the single `reqwest-rustls` entry is what
  keeps `cargo tree -i native-tls` and `-i openssl-sys` empty and `reqwest` on one version.
- **`PlaybackContext.http` is the cell, not the client.** A transport command re-opening a paused
  station needs a pool and must not build a second; handing over the `Arc<OnceLock<…>>` means a
  player that never tunes in still never loads rustls.
- **A station is not a queue entry** (D9) and pausing one drops its connection. The queue is
  track-id-based end to end and a station has no track id, so starting one stops queue playback and
  leaves the queue seated underneath — which is what hands the library back on stop. `queue.json`
  holds the station's row id *beside* the queue for the same reason.
- **Nothing logs a stream URL.** It can carry a session token, so `PlayerAction::PlayStream` renders
  its generation rather than its URL and `player::stream_source` names the station in every message
  and the URL in none. `RadioStationRow` deliberately has no `stream_url` field at all; it crosses
  on `RadioVm` and `Radio.detail-stream-url`, the two places something actually needs it.

## Numbers both trees spell

Each of these is written once in Rust and again in `.slint`, so each needs a pin or an argument.

- **`media::station_logo::MIN_LOGO_DIM` is restated in `components/now-playing/source-artwork.slint`**,
  which argues from it that no source reaching that tile can be small enough for `ArtworkImage`'s
  inset arm to fire — so it binds no `native-size`. Lower the floor and the argument stops being
  true with nothing to say so. Pinned by
  `media::station_logo::tests::the_slint_tile_that_skips_native_size_still_agrees_with_the_floor`.
- **`Radio.tab-count` is the sole definition of how many tabs there are.** `seed_tab` clamps the
  persisted index against the global's own count rather than a Rust const, `RadioTab` carries one
  variant per tab, and `tab_from_index` ends in a default arm so a fourth tab resolves to Browse
  rather than to nothing. All four halves are pinned in `ui::radio::tests`.
- **`NAV_RADIO` is 10 and `MAX_NAV_INDEX` is the same 10**, the latter bounding the persisted nav
  index at *both* ends of its round trip. They were literals once, and disagreeing literals are
  what made Radio persist as Settings and boot onto My Library — silent at both ends.
- **`id == 0` means "no database row"**, given its one meaning in `rows::station_has_row` and asked
  from four directions: which cache a page resolves from, whether a removal has a target, whether
  `views.json` may name the page (D6), and whether a history walk may reopen it.
- **The `-1` sentinels are declared twice by hand** — `NO_SEAT`, `VOTES_UNKNOWN`, `chip_index`'s
  miss, and `tab_bar::UNFETCHED_COUNT`. The facet chip *indices* are the one group that is safe:
  `facets::chip_indices` reads them off the global rather than restating them.
