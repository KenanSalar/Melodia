# Internet Radio

Working doc for [#29](https://github.com/KenanSalar/Melodia/issues/29). Delete when the feature ships.

Upstream facts verified **2026-08-20** against crate sources, the live API and this tree.
Anything marked ⚠️ **re-verify** is expected to drift; check it on the day rather than
trusting this doc.

---

## Context

Melodia plays files on disk and nothing else. There is no way to type a stream URL, no
directory to browse, and nothing in the sidebar between My Library and Settings. A user
who wants a station keeps a second app open for it.

What that costs is not only the stations. Every player-adjacent feature already built
(equalizer, visualizer, media keys, tray, sleep timer, Now Playing, Discord presence)
stops at the edge of the local library, so the app looks complete right up to the moment
someone wants a live source.

Issue #29 also leaves one question open: where stations come from. This doc answers it.

## Scope

**In:** a Radio section with two tabs (Browse and Favorites) and a station detail behind
both, a worldwide station directory, user-added stream URLs, live playback through the
existing DSP chain, live track titles off the stream, favorites and station history, the
OS integrations a stream can honestly support, and a settings toggle that removes the
whole feature for users who want to stay local only.

**Out, and tracked elsewhere:** podcasts (#30), commercial or self-hosted streaming
services (#31), Opus decoding (#35, which this feature benefits from but does not block on).

**Deliberately deferred inside this feature** (argued in "Deferred", at the bottom):
stream recording, HLS stations, radio scrobbling, Chromecast, alarm clock.

---

## Prior art

Surveyed before designing, per the usual order. The point of the table is which features
are load-bearing across independent implementations, not which app is best.

| App | Directory | Add by URL | Favorites | Live title | Song history | Record | Notes |
|---|---|---|---|---|---|---|---|
| **Shortwave** (GNOME) | radio-browser | yes | yes ("library") | yes | yes | yes | Radio-only app, the most complete reference for UX |
| **RadioDroid** (Android) | radio-browser | yes | yes | yes | yes | yes | Adds alarm clock, sleep timer, MPD output |
| **Tuner** (elementary) | radio-browser | no | yes (star) | yes | no | no | Minimal, discovery by genre/country/popularity |
| **Strawberry** | radio-browser + Radio Paradise + SomaFM | yes | yes | yes | no | no | Closest peer: a full library player that added radio |
| **Clementine** | Icecast + SomaFM + several curated | yes | yes | yes | no | no | Strawberry's ancestor, the curated lists have rotted |
| **Audacious** | none built in | yes | via playlist | yes | no | no | URL-only, radio is just another playlist entry |
| **foobar2000** | none built in | yes | via playlist | yes | no | components | URL-only in the core |
| **VLC** | Icecast directory | yes | no | yes | no | yes | Directory is an afterthought |
| **Rhythmbox** | small bundled list | yes | yes | yes | no | no | The bundled list is the weak part |

**What every serious implementation has**, and therefore what this plan treats as the
baseline: a directory backed by [radio-browser.info](https://www.radio-browser.info), add
by URL, local favorites, live titles parsed from the stream, a buffering indicator, and
automatic reconnect.

**What the good ones add:** search by name plus filters for country, language, tag, codec
and bitrate; popularity ordering; a station page with homepage link and tags; station
play history; voting back to the directory; a station logo in Now Playing.

**What only radio-first apps add:** recording, alarm clock, Chromecast. All three are
deferred here, with reasons.

### Why radio-browser.info

The alternatives were considered and lost:

- **A bundled station list.** What rotted Clementine's radio. Curation is a maintenance
  tax paid forever, and it cannot satisfy "any radio in the world".
- **Icecast's own directory** (`dir.xiph.org`). Only stations that opted into Icecast's
  yellow pages, a small fraction, heavily skewed to hobby streams.
- **URL entry only.** What Audacious and foobar do. Cheapest to build, and it makes the
  feature useless to anyone who does not already know a URL.
- **radio-browser.info.** Over 60,000 stations (`/json/stats` reported 62,689 on
  2026-08-20), community-maintained, no API key, no
  account, CC0 data, a documented mirror list, and it is what Shortwave, RadioDroid,
  Tuner and Strawberry all standardised on. It also serves `url_resolved`, so its own
  stations arrive already resolved past `.pls`/`.m3u` indirection.

The one thing it asks in return is a descriptive `User-Agent` of the form
`appname/appversion`. `services::build_http_client` already sends `Melodia/<version>`,
so the shared client satisfies it with no change.

---

## Decisions

Each of these closes a fork that would otherwise be re-argued mid-phase.

**D1. The directory is radio-browser.info, reached through a session-pinned mirror.**
Server discovery is documented as a DNS SRV lookup of `_api._tcp.radio-browser.info`,
which would mean a resolver crate. `GET https://all.api.radio-browser.info/json/servers`
returns the same list over HTTPS, so the client picks one at random per session and falls
back to a hard-coded host if that call fails. No new dependency.

⚠️ **re-verify.** The multi-mirror era this design assumes is over: on 2026-08-20 both SRV
and `/json/servers` return exactly one target, `de1.api.radio-browser.info`, and
`all.api.radio-browser.info` resolves to that same machine. So the random pick buys
nothing today and the design is kept only because the set can grow back. Two things it
must get right regardless, and both are invisible while the list has one entry:

- **Entries are `{"ip", "name"}` and the list carries one entry per address family.** Today
  that is `de1` twice, IPv4 and IPv6. Dedupe by `name` before picking, or a "random" pick
  is a coin flip between two spellings of one host.
- **Dial the `name`, never the `ip`.** The certificate is issued for the hostname, so an
  IP-keyed request fails TLS verification. The `ip` field is there for clients doing their
  own resolution, which is exactly what this design avoids.

Hard-coded fallback: `de1.api.radio-browser.info`.

**D2. Stations live in SQLite, in one table.** Favorites, user-added URLs and play
history are the same row at different stages of its life, so one `radio_stations` table
with `is_favorite`, `play_count` and `last_played` covers all three. A JSON file was the
alternative; it loses ordering, counting and the artwork join, and the app already owns a
database for exactly this shape of data.

**D3. Directory results are never persisted.** They are a network answer with a shelf
life, cached in memory for the session and dropped on section leave, like every other
grid cache in the tree. A row crosses into SQLite only when the user favorites it or
plays it.

**D4. Nav index 10, sidebar row directly under My Library.** 4 through 7 are retired and
8 and 9 are already in `views.json` on every install, so 10 is the next free index.
Sidebar order is source order, so the row mounts after My Library and before the Settings
divider regardless of index value.

**D5. Two tabs on one `LibraryTabBand`, which morphs into a station hero on drill-in.**
Browse and Favorites, with a station detail behind both. The band already mounts
`TabSearchHeader` internally, so the flat idle state is the same row the Settings page
wears; what it adds is the morph into a hero when a detail opens. Its artwork square is a
plain `ArtworkImage` taking `artwork-path`, `fallback-icon`, `tile-bg` and
`tile-icon-color`, which is exactly a single station logo's shape, unlike `MosaicTabHero`,
whose `MosaicHeroTile` is built around a composed collage.

Two tabs on one nav index, drilling into a detail that morphs the band rather than
routing, **is My Library's shape exactly**, which also settles the back arrow: it means
"close this detail" and stamps `origin-nav-index = -1`, for the same reason it does there.
Restoring an origin would contradict the tab bar sitting beside it.

**D6. A station detail is opened from a row, not fetched by id, and only persists when it
has one.** `views.json`'s `last_detail_ids` is a `HashMap<String, i64>` of database row
ids, and per D3 a directory station that has been browsed but never favorited or played
has no row. So the open call takes the whole `RadioStationRow`, which Browse already holds
in its model, and the restart seed applies only when `id != 0`. A directory-only detail is
transient and a restart lands on the tab root.

Upserting a row on every drill-in was the alternative. It makes persistence uniform and
costs a growing table of stations the user merely glanced at, with "seen" and "kept"
indistinguishable in the same rows. One conditional beats that.

**D7. The hero gets a source-size floor, because station logos are the weak part.**
radio-browser favicons are routinely 32 or 64 px, often `.ico`, and often dead links.
`Theme.hero-artwork` is a large tile and `HeroBackdrop` derives its blur seeds and palette
from that same image, so a tiny source gives both an upscaled tile and a poor quantization
sample. Below a minimum stored dimension the image tier is skipped outright and the band
takes its `fallback-icon` path with the `radio` glyph, which it already supports. This is a
rule written down here rather than a surprise discovered on screen.

`.ico` is a second half of the same problem: `image` is compiled with `jpeg`, `png`,
`webp`, `gif`, `bmp` and `tiff` and no `ico`, and the feature list has to stay the superset
of `media::artwork`'s `STORED_EXTENSIONS`. Either add the feature to both or skip `.ico`
favicons; decide it in the phase that downloads them.

**D8. The network never touches the audio callback thread.** rodio pulls `Source::next()`
inside the cpal data callback (`rodio-0.22.2/src/stream.rs`, `init_stream`), so a blocking
socket read would stall the whole mixer, local track included, for as long as the network
is wedged. A decoupling ring buffer with its own feed thread sits between the decoder and
the DSP chain, yields silence when starved, and publishes that starvation as the
buffering state the UI already needs. This is a requirement, not a refinement.

**D9. A station is not a queue entry.** The queue is track-id-based end to end
(`PersistableQueue { track_ids: Vec<i64> }`), and a station has no track id. Starting a
station stops queue playback and leaves the queue untouched, so returning to the library
resumes where it was. Next and Previous are disabled while a station plays, which is what
Shortwave, Tuner and RadioDroid all do.

**D10. Crossfade, gapless and ReplayGain are off for streams; EQ, limiter and visualizer
are unchanged.** The first three are per-track transitions and per-track tags, neither of
which a live stream has. The last three ride the shared `EqSource` chain and work for free.

**D11. Playback speed is pinned to 1.0 while a station plays.** rodio implements speed by
reporting a multiplied sample rate upward, which against a fixed-rate live source drifts
the buffer until it starves. The control is disabled rather than silently ignored.

**D12. Station logos go through the shared artwork store, and the sweep's reference set
grows to five columns.** `tasks::artwork_sweep` deletes any stored file no column names,
so adding `radio_stations.artwork_path` to that ledger is not optional; miss it and every
station logo is deleted on the next sweep.

**The ledger is two consts in `src/database/queries/artwork.rs`, and both are edited.**
`ARTWORK_COLUMNS` (`pub(super)`, `[(&str, &str); 4]`) is the `(table, column)` list
`repoint_all` builds its statements from; `REFERENCED_PATHS` is the `UNION` the sweep
reads through, spelling the same four out by hand. That is deliberate rather than drift,
argued in the doc comment on `ARTWORK_COLUMNS` and pinned by a test, so the fifth column
lands in both and the pin is what catches a half-edit. Both names are `artwork.rs`-local,
so grep the file rather than the crate.

**D13. HLS stations are surfaced as unplayable, not hidden.** Symphonia has no MPEG-TS
demuxer, so an HLS station cannot play today. The directory row carries `hls: 0|1`, so
the card can say so plainly instead of the station failing at the moment of a click. A
filter defaults to hiding them, with a toggle.

**D14. The Radio Browser client is a service, and the UI's only door is `library::radio`.**
`src/ui/` reaching `crate::database` directly is already forbidden and the same logic
applies to a network directory: one facade in `library/` fronts both the local table and
the remote search, so a callback never has to know which side answered.

**D15. One toggle removes the whole feature, and it is enforced at the facade, not in the
UI.** A user who wants Melodia local-only can turn radio off, and the sidebar row at index
10 stops being mounted. Scoped to this feature alone: it gates nothing else, and the
existing scrobbling and Discord toggles are untouched by it.

The gate that matters is not the one on the sidebar. What that user is buying is "no
traffic", so the guard belongs where traffic originates: an early return in
`library::radio`, which D14 already made the single door for every directory call and
every logo download. One place to enforce it, and a grep can prove it. The UI gates are
cosmetic on top of that, and both are needed anyway so that nothing mounts and fetches.

**Default off**, for a reason rather than out of caution: the package description already
shipped to every distro says the collection stays on your own machine "with no accounts,
streaming or cloud", and `discord_rpc_enabled`, the nearest analogue in the tree, defaults
off for the same reason. The upgrade path is then silent, and the toggle is what turns it
on. That description text needs a pass either way (Phase 10).

**No restart.** Enabling mounts the row and the section immediately, so the slice installs
unconditionally at boot and only the network is gated. Disabling has three consequences
beyond the row disappearing, and all three are the phase's real work: a station currently
playing is stopped, `Nav.selected-index` is moved off 10 if that is where the user is
standing, and a persisted `last_nav_index` of 10 is folded on read at the next boot, the
way `fold_retired_nav_index` already folds 4 through 7.

---

## Structure

### New files

| Path | Owns |
|---|---|
| `migrations/20260820000000_radio_stations.sql` | The one table. Branch-local until merge, so fold changes into it rather than adding a second |
| `src/entities/radio.rs` | `RadioStation` (the row), `NewRadioStation` (the save input), and the directory's boundary types: `DirectoryStation`, `Facet`, `FacetKind`, `StationSearch`, `SearchOrder`. No Slint-shaped DTO: that is `models.slint`'s, per Phase 1's note |
| `src/database/queries/radio.rs` | Every SQL statement touching `radio_stations` |
| `src/library/radio.rs` | The facade: favorites, custom stations, history, and the directory calls the UI needs |
| `src/services/radio_browser/mod.rs` | Mirror discovery, the shared client's request builder, error mapping |
| `src/services/radio_browser/model.rs` | The API's JSON shapes, deserialized once |
| `src/services/radio_browser/query.rs` | Search and facet-list request construction, paging |
| `src/player/stream_source.rs` | Opening a URL into a decodable reader: stream-download, ICY, playlist resolution |
| `src/player/prebuffer.rs` | The ring buffer and its feed thread (D8) |
| `src/ui/radio/mod.rs` | Slice install, the `RadioUi` handle, row and cover plumbing |
| `src/ui/radio/tabs.rs` | `RadioTab`, the tab resolve, the persisted-tab seed |
| `src/ui/radio/detail.rs` | The station detail: open, close, hero artwork, the restart seed |
| `src/ui/radio/callbacks.rs` | Browse, Favorites, detail and playback wiring. A directory once it outgrows one file, as Favorites' and Recently Played's did |
| `melodia-ui/ui/globals/radio.slint` | The `Radio` global |
| `melodia-ui/ui/views/radio-view.slint` | Page chrome only: the band and tab routing. **No page-level scroller** — every tabbed page in the tree puts it in the tab body, and Browse's grid is a virtualized `ListView` a page `ScrollView` above it would fight |
| `melodia-ui/ui/views/radio/browse-tab.slint` | Browse tab body |
| `melodia-ui/ui/views/radio/favorites-tab.slint` | Favorites tab body |
| `melodia-ui/ui/views/radio/station-detail.slint` | The detail body under the morphed band |
| `melodia-ui/ui/views/radio/station-card.slint` | One station, shared by both tabs |
| `melodia-ui/ui/components/dialog/add-station-body.slint` | The add-by-URL dialog body |
| `melodia-ui/ui/views/settings/radio-section.slint` | The Settings card: the master toggle and its sub-rows |
| `.claude/rules/radio.md` | The contract, once it spans Rust, `.slint`, a migration and the packaging deps |

### Edited files, and what each edit is

| Path | Edit |
|---|---|
| `Cargo.toml` | Two dependencies, both feature-pinned (see Phase 3), and possibly `image`'s `ico` (D7) |
| `melodia-ui/ui/layout/sidebar.slint` | One `SidebarItem` after My Library, behind a `Divider` — everything above it is the local library and this is the one row that reaches the network |
| `melodia-ui/ui/globals/nav.slint` | The index map, which is authoritative and now runs to 10 |
| `melodia-ui/ui/app-window.slint` | The `export { }` name list, one `ViewTransition` branch, the `!= 10` term on the placeholder fall-through, one `SectionActiveGate` mount, one `view-title` arm |
| `melodia-ui/ui/models.slint` | `RadioStationRow`, plus the live fields on `PlayerVm` |
| `melodia-ui/ui/settings.slint` | `radio-enabled` and its changed callback on the `Settings` global |
| `melodia-ui/ui/views/settings/pages/services-page.slint` | A third card, and one more term in `has-matches` |
| `src/services/settings/data.rs` | `RadioFlags`, flattened into `SettingsData`, defaulting off |
| `src/boot/ui_setup.rs` | The nav-index guard, the tab seed, the slice install; later, folding a persisted 10 when radio is off |
| `src/services/view_state.rs` | `radio_tab: i32`, and `MAX_NAV_INDEX` — the bound the persisted nav index's write clamp and read guard now share |
| `src/library/settings/view.rs` | `set_radio_tab`, and `set_last_nav_index`'s clamp off that const |
| `src/ui/view_tag.rs` | The nav-10 arm, naming the tab like the other three tabbed pages |
| `src/test_support.rs` | `CALLBACK_HOMES` grows to 13 |
| `src/database/queries/artwork.rs` | The fifth reference column (D12) |
| `src/player/state.rs` | The radio arm of the state machine |
| `src/player/types.rs` | `RadioNowPlaying`, the live source's answer to `TrackSummary` |
| `src/player/rodio_backend.rs` | `play_stream` / `stage_stream`, and `build_source` made generic over its source |
| `src/player/handlers.rs` | The monitor's live-source arms |
| `src/library/playback.rs` | `player_play_station`, and the radio branch on play / toggle |
| `src/state/contexts.rs` | `PlaybackContext.http`, so a transport command can re-open a station |
| `scripts/icons.txt` | `radio` and whatever else the cards use |
| `melodia-ui/translations/*/LC_MESSAGES/melodia-ui.po` | Every new msgid, in all six |

### What must not grow

- **`src/player/rodio_backend.rs` does not learn HTTP.** It takes a reader that is already
  open. Everything network-shaped lives in `stream_source.rs`.
- **No second `decode_file`.** `build_source` is generic over `S: Source`, and both callers
  hand it what they have — a `Decoder` for a file, a `PrebufferSource` for a stream, since
  D8's ring sits between the stream's decoder and the DSP chain. A parallel
  `build_stream_source` is the duplication this note exists to prevent. The two *decoder*
  constructions do stay separate, answering different questions (extension hint and byte
  length against response mime type and never seekable).
- **No *third* reference-column ledger.** D12 is the existing pair, `ARTWORK_COLUMNS` and
  `REFERENCED_PATHS`, both grown by one. Not a new list, and not a query that re-derives
  them somewhere else.
- **No second "does this row match" predicate.** The Favorites filter goes through
  `src/ui/row_match.rs` like the other sixteen surfaces.
- **No second tab bar, and no third band.** `LibraryTabBand` is mounted, not copied, and
  not parameterised into a shared ancestor with `MosaicTabHero`. The two are siblings for
  reasons `ui-patterns.md` argues, and `ui::library_tab_band_tests` holds them to it.
- **No second HTTP client.** `services::build_http_client`'s pool is process-wide and its
  User-Agent is what the directory asks for. The stream path needs an `Icy-MetaData: 1`
  default header that the directory and favicon paths should not send, and that is a
  newtype implementing stream-download's `Client` trait over the shared client (Phase 3),
  not a second `reqwest::Client`. A bare `StreamDownload::new_http` is the regression this
  note exists to prevent: it constructs its own unconfigured client behind your back.

---

## Cross-cutting checklist

The things that are silent when missed. Each is checked off in the phase that owns it.

- [x] `radio_stations.artwork_path` in **both** `ARTWORK_COLUMNS` and `REFERENCED_PATHS`
      in `src/database/queries/artwork.rs`, and the array widened to `; 5]` (D12).
- [ ] The logo fetch compares `favicon_url` before trusting `artwork_path`. A re-import
      refreshes the URL and deliberately keeps the stored file, so a station whose logo
      moved otherwise shows the old one forever (Phase 3).
- [x] **One** `SectionActiveGate` at `index: 10`, not one per tab. Amended in Phase 4 and
      argued there: the two tabs share a handle, as Favorites' three and Recently Played's
      two do, and a per-tab gate would make a tab flip a section leave — handing back a
      directory answer the session paid a network round trip for.
- [x] The persisted nav index is written **before** `wire_all`, so the slice's
      `section_active` shadow seeds correctly. Same for `seed_tab` (Phase 4).
- [x] `radio_tab` clamped on read through `ui::tab_bar::clamp_tab` against
      `Radio.tab-count`, never a Rust const (Phase 4).
- [x] The persisted nav index survives a round trip. `set_last_nav_index` clamped writes to
      `0..=9` and `install_views` guarded reads with the same literal, so a Radio index was
      rewritten on the way out *and* dropped on the way in. Both read
      `services::view_state::MAX_NAV_INDEX` now (Phase 4).
- [ ] Counts hold `UNFETCHED_COUNT` until fetched, and the section leave puts them back.
- [ ] The band's `hero-t` is **written** from `changed detail-open` and only seeded by its
      binding, so a page entered with a detail already open lands at hero height.
- [ ] The hero's facts outlive the detail id: teardown rides `hero-collapsed()`, not the
      id going away, or the band paints an empty hero through the whole collapse.
- [ ] A drill-in lands its navigation inside `open_*_with`'s `on_applied` hook, never up
      front, so the id and the navigation arrive in the same tick.
- [ ] `last_detail_ids` is written only for a station with a row (D6).
- [ ] The disabled guard is in `library::radio`, not only in the UI, and a grep for the
      Radio Browser client outside that facade returns nothing (D15).
- [ ] Disabling stops a playing station, moves `Nav.selected-index` off 10, and folds a
      persisted 10 on the next boot.
- [ ] The `SectionActiveGate` mount stays mounted when radio is disabled. It carries a
      `changed` tracker, and dropping a tracker-bearing branch panics.
- [ ] The Settings sub-rows sit under `if Settings.radio-enabled`, never `visible: false`
      (slint#7377), matching the crossfade rows.
- [ ] The new Settings rows register their haystacks, or the page's search cannot find them.
- [ ] Every new `@tr` msgid lands in all six catalogs. The catalogue walk test fails
      otherwise, and a miss ships silently as the English msgid.
- [ ] Every new icon name is in `scripts/icons.txt`, the fonts are re-subset, and
      `scripts/check-icons.py` passes. A missing name renders as tofu.
- [x] `cargo tree -i native-tls` and `cargo tree -i openssl-sys` are both empty after the
      dependency add, and `cargo tree -i reqwest` shows one version. `stream-download`'s
      default features pull `reqwest/default-tls`, which is OpenSSL on Linux and changes
      what `cargo-deb`'s `$auto` resolves to. Held by `default-features = false` plus the
      single `reqwest-rustls` entry (Phase 3).
- [x] Every directory filter's parameter **name** is pinned by a test asserting a *set*
      value lands under that exact key. An unknown query parameter is dropped silently, so
      a misspelled filter reads as a working one, and an absent-when-blank assertion is
      satisfied by the misspelling too. Asserting that an excluded *value* is absent would
      take a live call and the tree has no network tests, so the observed evidence for
      `bitrateMin` lives in that test's doc comment instead (Phase 2).
- [x] Nothing calls `StreamDownload::new_http`; the stream goes through `HttpStream::new`
      with the shared-client newtype, or the ICY header and the User-Agent are both lost.
      Held by a corpus walk over `src/`, since the temptation is a one-line constructor.
- [x] A rebuffer does not flip MPRIS to Stopped or clear Discord presence. Settled by
      leaving `Loading` to mean the initial connect and raising `radio.buffering` beside a
      `Playing` status, so neither reporting site needed an edit (Phase 3).
- [ ] No `unwrap()`, no `#[allow(dead_code)]`, no `sed`-driven edits.
- [x] Thread names stay under 15 bytes. The stream's feed thread is `radio-buffer`, and
      `services::tests::no_thread_name_outgrows_what_the_kernel_keeps` walks `src/` for it.
- [x] Nothing logs a stream URL that carries a token in its query string —
      `PlayerAction::PlayStream` renders its session number rather than its URL, and
      `player::stream_source` names the station in every message and the URL in none
      (Phase 3). Still open for the phases that download logos and fetch the directory.

---

## Phase 1: The table and the facade ✅ landed

Stations exist in the database and `library::radio` answers questions about them. No UI,
no network, no audio. The schema lives in `migrations/20260820000000_radio_stations.sql`
and is not restated here, a second copy being the drift this repo pays for most.

Shipped: the migration, `src/entities/radio.rs` (`RadioStation` + `NewRadioStation`),
`src/database/queries/radio.rs` (eight functions and its tests), `src/library/radio.rs`,
and the fifth artwork reference column in both halves of the ledger.

**Five deviations from this section as first drafted**, each argued at its anchor:

- **No `AUTOINCREMENT`.** There are zero in the tree; plain `INTEGER PRIMARY KEY` is the
  convention, and `AUTOINCREMENT` buys a `sqlite_sequence` write per insert that nothing
  here needs. It does mean a deleted station's id can be reused, so **`last_detail_ids`
  can land on a different station across a restart** (D6) exactly as it can for a
  playlist.
- **No secondary indexes.** `station_uuid TEXT UNIQUE` is index-backed and is what the
  upsert conflicts on. Favorites and recents scan a table the user fills by hand, where a
  scan beats a seek. `tracks` is the opposite shape and its history runs both ways:
  `idx_tracks_last_played` was dropped there as write-only and had to come back once
  Recently Played existed. Adding one here is additive, so it waits for the surface that
  wants it.
- **One `save_station`, not an upsert plus an insert.** The `station_uuid` already says
  which is wanted, so the conflict clause is chosen from it rather than by the caller
  picking between two functions with identical signatures, one of which fails a UNIQUE
  constraint when handed a directory row.
- **`hls BOOLEAN` added.** D13 promises an "unplayable" badge and Phase 7 favorites a
  directory row into this table, so without the column a kept HLS station is stored
  indistinguishable from a playable one.
- **No `RadioStationRow` yet.** Nothing in `src/entities/` references Slint; the `*Row`
  structs are the generated ones from `models.slint` and all 16 `to_slint_*` converters
  live in `src/ui/<view>/mod.rs`. **Phase 4 owes both halves**: the struct in
  `models.slint` and `to_slint_radio_station_row` in `src/ui/radio/mod.rs`.

**Two things Phase 5 inherits.** `library::radio` is already the single door D15's guard
needs, and it deliberately does not bump `library_changed_tx`, no library view showing a
station.

**Gates run:** clippy, fmt, full `cargo test` (1792 unit + 16 integration, green). The
ledger pin was mutation-checked: dropping the radio arm from `REFERENCED_PATHS` fails both
`the_reference_query_names_every_artwork_column` and
`a_logo_referenced_only_by_a_station_is_still_referenced`.

---

## Phase 2: The directory client ✅ landed

`library::radio::search(...)` returns stations from radio-browser.info and
`library::radio::facets(...)` the four lists Phase 6's filter chips are built from. Still
no UI and no audio.

Shipped: `src/services/radio_browser/{mod,model,query}.rs` with their three test files,
the five boundary types in `src/entities/radio.rs`, and the two facade functions.
**No new dependency** — `reqwest`'s `json` and `query` features, `serde_json`, `rand` and
`tokio`'s `sync` were all already direct deps.

What later phases reach for:

- **`DEFAULT_PAGE_LIMIT`, `TAG_FACET_LIMIT` and `FACET_LIMIT`** in `query.rs`, each argued
  at its definition. Paging is `StationSearch::offset` plus `limit`.
- **`DirectoryStation::to_new_station()`**, Phase 7's bridge from a browsed station to a
  kept one. It passes the uuid across as a plain `Some`, which is only sound because
  `search` drops anything failing `DirectoryStation::is_usable` first: an empty uuid is a
  value rather than a gap to `station_uuid TEXT UNIQUE`, so uuid-less stations would upsert
  onto one row. The same predicate keeps a station with no stream URL off the grid, which
  is what an upstream rename of `url`/`url_resolved` would otherwise produce silently. A
  client-side drop shortens a page, the same as D13's `hls` filter below.
- **Facet lists cached per session** in four `tokio::sync::OnceCell`s and handed out as
  `Arc<[Facet]>`, so re-entering the section costs nothing.
- **`Facet::code` filters only for countries.** `countrycode` is the search endpoint's one
  code-keyed parameter; languages carry an `iso_639` and still filter by `name`, as tags
  and codecs do with no code at all. Phase 6's chips have to split on that, or a language
  chip sends `language=en` and substring-matches english, armenian and slovenian alike.
- **A directory call is bounded by `REQUEST_TIMEOUT`**, per request rather than on the
  shared client, whose per-read deadline is what the updater's downloads want and no bound
  at all on a fetch somebody is waiting on.
- **`bitrate` is `0` on a large share of live stations**, the most-clicked station in the
  world included, so it is a display hint and never an input to a calculation without a
  fallback. Phase 3's prefetch sizing is the caller that would otherwise divide by it, and
  it should lead with `IcyHeaders::bitrate()` rather than this field.

**Four things the live API does that this section did not say.** Each was verified against
`de1.api.radio-browser.info` and each is now pinned by a test:

- **`/json/tags` returned 1000 entries with no `limit`** against 11,943 tags, and orders
  alphabetically, so the first entry it returns is `"bob"`. The published default for the
  list endpoints is 100,000, so that ceiling is the live server's rather than the
  documented one, which is reason to send a limit rather than to trust either. "`limit` is
  not optional in practice" is
  therefore true of the facet endpoints too, and a usable tag list needs
  `order=stationcount&reverse=true`. Countries (241), languages (649) and codecs (11) fit
  whole; tags take the popular head.
- **`topvote` and `topclick` are `search` with an `order`.** `order=clickcount&reverse=true`
  returns the identical head, so the two extra endpoints this section listed were dropped
  rather than built: one request builder, one response path, one set of tests.
- **Station names arrive padded** (`"\tArrow Classic Rock"`), and the directory holds
  duplicate stations under distinct uuids. Trimming is in the wire-to-entity conversion.
- **There is no `hls` search parameter**, so D13's filter is client-side. This phase
  carries the field; Phase 6 owns the filter, and inherits that a client-side drop
  shortens a page.

**Three deviations from this section as first drafted:**

- **The directory answers with `entities::radio::DirectoryStation`, not the wire struct.**
  It carries the facts the table has no column for (`votes`, `click_count`, `state`, the
  country's full name, `last_check_ok`) and none of the six that mean nothing before a row
  exists. `services::radio_browser::model` stays private, which is what keeps `src/ui/`
  from ever naming it (D14). `ssl_error` is deliberately not carried: nothing filters on
  it yet, and `hidebroken=true` already excludes most of what it would catch.
- **Direction belongs to the order, so `StationSearch` has no `reverse`.** Every
  popularity order wants the top of the list and alphabetical wants A, so a caller-set
  flag could only ever go one way per order; and `Default` would have had to contradict
  itself, claiming most-clicked-first while `bool::default()` said ascending.
- **Click and vote are not here.** Nothing calls them until Phase 6 (play) and Phase 8
  (the detail's vote action), and each wants its opt-out landing beside it. Their failure
  shapes are verified and **asymmetric, which is the part worth carrying forward**:
  `/json/url/{uuid}` reports an unknown station as **HTTP 404 with a zero-length body**,
  so it is checked on status and never parsed, while `/json/vote/{uuid}` reports one as
  **HTTP 200 with `{"ok":false,"message":…}`**, so it is checked on the body and never on
  status. Both are deduplicated server-side, a click once per IP per station per day and a
  vote once per ten minutes, so neither needs a client-side debounce and a repeated call
  is not an error to report.

**What Phase 5 inherits.** Every directory call and the facet cache sit behind the two
facade functions, and `only_the_radio_facade_reaches_the_directory_client` walks `src/`
to hold that, so D15's guard is one early return per function. There is deliberately no
`directory_client` seam yet: a helper that can only return `Ok` trips
`clippy::unnecessary_wraps`, so it arrives with the guard that gives it a reason to be
fallible.

**One thing left open.** A mirror that dies *after* discovery is pinned for the session,
`MIRROR` being a `OnceCell` with no re-derivation on repeated failure. `mirror()`'s doc
argues the no-retry case for a failed *discovery* and not this one; whether it is worth a
re-pick belongs with the phase that first has a user watching a spinner.

**Gates run:** fmt, `clippy -p Melodia --all-targets --locked -- -D warnings`, full
`cargo test` (1830 unit + 16 integration, green; 38 new). `cargo tree -i native-tls` and
`-i openssl-sys` are both empty and `reqwest` resolves to a single copy, which is the
baseline Phase 3's dependency add has to preserve. Five pins were mutation-checked:
spelling `bitrateMin` lowercase fails `the_minimum_bitrate_filter_is_camel_case`; spelling
`countrycode` camelCase fails `every_set_filter_lands_under_its_own_wire_key` and nothing
else, which is the whole reason that test exists beside the blank-filter one; a constantly
`true` `is_usable` fails both station-drop tests; a `radio_browser` mention planted in
`library/playback.rs` fails the reach walk; and dropping the dedupe in `model::hosts`
fails both `both_address_families_of_one_mirror_collapse_to_one_host` and
`distinct_mirrors_all_survive`.

---

## Phase 3: Stream playback ✅ landed

A station's URL plays through the existing DSP chain, with buffering, reconnect and live
titles. Still no UI: the first hand-test is Phase 6, so this phase's gate was its unit
suite rather than a listen.

Shipped: the two dependencies, `src/player/prebuffer.rs` and `src/player/stream_source.rs`
with their test files, `RadioNowPlaying` in `player/types.rs`, the radio arm of the state
machine and its three session builders, `PlayerAction::PlayStream` and its executor arm,
the monitor's live-source arms, `library::playback::player_play_station` and
`library::radio::play_station`.

What later phases reach for:

- **`PlayerState.radio: Option<RadioNowPlaying>` is the whole "is this a live source?"
  test**, and every transport builder branches on it. `current_track` is `None` throughout,
  so a surface reads whichever of the two is `Some` rather than a flag saying which to
  trust. Phase 9 draws it; the buffering spinner is `radio.buffering`.
- **`radio_generation` is bumped by every transition that starts or ends a session**, and a
  connect that finishes after the user moved on is refused by it. Anything later that ends
  a station (Phase 5's kill switch) goes through `build_stop_actions` and inherits that.
- **`library::radio::play_station(state, id)` is the door**, and it counts the play even
  when the stream turns out to be unreachable — the recents list records what the user
  chose. A directory station with no row needs a sibling; Phase 6 owns which.
- **`PlaybackContext` grew a sixth field**, the shared `Arc<OnceLock<reqwest::Client>>`, so
  a transport command can re-open a paused station without a second pool. `AppState::
  http_client_cell()` hands over the cell rather than the client, so a player that never
  tunes in still never loads rustls.

**Six deviations from this section as first drafted**, each argued at its anchor:

- **The ring sits between the decoder and `EqSource`, so `build_source` went generic over
  the *source*, not over a reader.** Step 4 as drafted assumed the stream path hands
  `build_source` a `Decoder<R>`; with D8's ring in the chain it hands a `PrebufferSource`,
  and the two only meet at `Source`. Smaller change, and it still satisfies "no parallel
  `build_stream_source`". `decode_file` and the stream's decoder construction stay two
  functions because they answer different questions: file open, extension hint, byte length
  and seekable, against mime type off the response and never seekable.
- **`Loading` means the initial connect and nothing else, so step 7's "status `Loading`
  throughout [reconnect]" was dropped.** Step 5 chose a flag beside `Playing` and the two
  contradict; step 5 wins. Every gap after the first audio — starvation or reconnect — stays
  `Playing` with `radio.buffering` raised. **Neither `services/media_controls/mod.rs` nor
  `services/discord/model.rs` was touched**, which is the point. (Their `Loading` arms were
  dead until now: nothing in the tree set it, so `discord/model.rs`'s comment about track
  changes passing through it was already stale and is now true for stations.)
- **Reconnect lives in the feed thread, not the monitor.** That thread already holds the
  URL, the client and the ring, so it re-opens and keeps filling the *same* ring: the rodio
  source never ends, the deck never blinks, and the state machine needs no reconnect path.
  The alternative would have handed `player/handlers.rs` an HTTP client and inverted the
  `library` → `player` dependency direction. The monitor's end-of-stream arm only sees a
  station that has already spent its budget.
- **Pausing a station drops its connection.** `stream-download`'s bounded storage pauses
  its *writer* when the reader falls behind, so a held-open socket back-pressures the server
  and resumes on stale audio. Pause keeps the station on screen with a play button that
  re-opens it — `library::playback::needs_station_reopen` is the pure predicate that routes
  it, shared with `player_play` so the two can't disagree. Stop forgets the station outright,
  which is what hands the untouched queue back (D9).
- **`buffering` lives on `RadioNowPlaying`, not on `PlayerState`.** It means nothing without
  a station, and putting it there kept `PlayerState` under the `struct_excessive_bools`
  threshold with no suppression. The view models carry it inside `radio` for the same reason.
- **Playlist resolution is written here rather than reusing
  `library::playlist_files::m3u`.** That parser is private to `library` (the wrong direction
  to reach from `player`) and answers a different question — track paths with BLAKE3 hashes
  and `#EXTINF` durations, none of which a stream playlist carries, and neither `.pls` nor
  `.asx` at all. One pass over the body covers all three formats.

**One thing left open.** A station whose feed thread gives up toasts its name through
`ToastKind::PlaybackFailed`, which is the same title a failed *track* gets. Phase 9 owns
the Now-Playing surfaces and is where a station-specific wording would land, if it earns one.

**Gates run:** fmt, `clippy --all-targets --locked -- -D warnings` at the root, full
`cargo test` (1874 unit + 16 integration, green; 44 new), and one `cargo build`.
`cargo tree -i native-tls` and `-i openssl-sys` are both still empty and `reqwest` still
resolves to a single copy, which was the baseline the dependency add had to preserve. Four
pins were mutation-checked: dropping the ring's whole-frame gate fails both
`a_partial_frame_is_never_split_across_the_ring_and_silence` and
`a_trailing_partial_frame_is_dropped_rather_than_played`; returning `None` instead of
`Some(0.0)` on starvation fails three prebuffer tests; dropping the radio arm from
`build_end_of_stream_actions` makes a station going off air start playing a library track
and fails `a_station_going_off_air_stops_rather_than_advancing_the_queue`; and a planted
`StreamDownload::new_http` fails the corpus walk.

---

## Phase 4: The section shell ✅ landed

The sidebar row, the page, its two tabs and the persistence. The tabs are empty — Browse
fills in Phase 6, Favorites in Phase 7, the band's hero half in Phase 8 — so what landed is
the shell and the boot ordering under it, which is the half that cannot be retrofitted.

Shipped: `melodia-ui/ui/globals/radio.slint`, `views/radio-view.slint` and its two tab
bodies, the sidebar row, the router branch and its `!= 10` term on the placeholder
fall-through, one `SectionActiveGate`, `src/ui/radio/` (`mod`, `tabs`, `callbacks`),
`radio_tab` in `views.json` with its setter, and the nav-index bound both halves of that
round trip now share.

**Two things silently defeated a nav index of 10, and fixing them was the first work item.**
`library::settings::set_last_nav_index` clamped a write to `0..=9` and
`boot::ui_setup::install_views` guarded the read with the same literal, so Radio persisted
as Settings and booted onto My Library. Both read `services::view_state::MAX_NAV_INDEX` now,
which lives beside `last_nav_index` and its default because a clamp and its guard
disagreeing is precisely the failure neither site can see on its own.

What later phases reach for:

- **`RadioUi.section` is the page's one `SectionState`**, seeded at wire time from
  `tabs::section_is_up` rather than left to the gate — that fires on transitions only and its
  `ChangeTracker` baselines silently inside `AppWindow::new()`, so a section seeded wrong has
  no edge left to correct it. The hook mirrors the flag and **nothing else**: a leave owes
  `mark_dirty` for exactly what it hands back, and this page holds nothing yet. Phase 6 adds
  the release and its `mark_dirty` together.
- **`Radio.tab-count` is the sole definition of how many tabs there are**, and `seed_tab`
  clamps the persisted index against it through `ui::tab_bar::clamp_tab`.
- **`tab_from_index` ends in a default arm**, so a third tab added to the global without one
  here resolves to Browse and `ui::view_tag` logs that.
- **The band's box is bound but has no Rust destination.** Typing filters nothing; a tab pick
  clears both sides Slint-side, the `recently-played-view.slint` shape. Phase 6 brings the
  `FilterThrottle`, `filter-changed` and the dispatch together.

**Five deviations from this section as first drafted**, each argued at its anchor:

- **One `SectionActiveGate`, not one per tab.** My Library mounts five because its five tabs
  *were* five sidebar sections with five hooks; Favorites' three tabs and Recently Played's
  two mount one each, sharing a handle, which is this page's shape. It also has to be one: a
  per-tab gate makes a tab flip a section leave, so glancing at Favorites would hand back a
  directory answer the session paid a network round trip for (D3). The cost is the
  `covers-generation` cold-tier gate on a tab pick, which Phase 6's logo tier owes anyway.
- **No page-level scroller.** Step 3's `ScrollView` under a root `OverlayScrollbar` would sit
  above Phase 6's virtualized station grid, which is the nested-scroller pitfall; every
  tabbed page in the tree puts the scroller in the *tab body* instead
  (`my-library/albums-tab.slint` is the 49-line model). The `page-w` mirror, the 1 ms mount
  `Timer` and the seed at the row's floor were never the page's either — they are
  `library-tab-band.slint`'s, and the mount inherits them.
- **No row models, no `RadioStationRow`, no `to_slint_radio_station_row`.** Step 1 and Phase
  1's closing note both put them here, and a converter with no caller is dead code this tree
  forbids. Each half lands with the surface that fills it: Browse's in Phase 6, Favorites' in
  Phase 7. The global carries only what this phase wires.
- **No count latch.** My Library holds its count line across a drill because the band eases it
  out over the morph's first half and a live binding re-reads the arriving tab mid-fade. With
  `detail-open` a literal `false` there is no morph to protect, so `count-text` binds the
  guarded ternary directly. Phase 8 owes the latch alongside the hero.
- **No `watched-tab-idx` mirror.** My Library's exists for arrivals that aren't picks — a
  cross-tab drill, a Mouse-4/5 walk — and for the filter reseat. This page has neither yet, so
  the bodies take `band.tab-anim-armed` directly, as Recently Played's do.

**Filter box routing, deferred to Phase 6 with its first destination.** The band carries a
single box and the two tabs want different things from it: on Browse a directory query,
debounced and sent over the network; on Favorites a local `row_match` filter. One dispatch
site in Rust routes a settled keystroke by the live tab, the way
`my_library/filter.rs::dispatch` routes its nine surfaces. Phase 8 adds a third destination,
which is why it is a function from the outset rather than an `if` that grows. What landed
here is the box, its per-tab placeholder, and the tab pick that clears it.

**The sidebar row and the router branch landed ungated**, and Phase 5 adds the `if` term to
each. Two one-line edits of churn, taken deliberately: the alternative is a phase whose only
deliverable is a toggle with nothing to toggle, which cannot be tested by hand.

**Gates run:** fmt, `clippy --all-targets --locked -- -D warnings` at the root (both crates
moved), full `cargo test` (1879 lib + 4 binary + 12 integration, green; **none added** —
this phase's pins wait for the manual pass), and `scripts/check-icons.py` after re-subsetting
for `radio` and `travel_explore`. `CALLBACK_HOMES` grew to 13, and the six catalogs gained
six msgids — the two tab labels reuse the sidebar's existing `"Browse"` and `"Favorites"`.

---

## Phase 5: The kill switch

**Goal.** Radio can be turned off completely, and off means no network, not a hidden row.
Scoped to this feature: nothing else in Settings is touched.

1. `RadioFlags { radio_enabled: bool }` in `src/services/settings/data.rs`, flattened into
   `SettingsData` with the struct-level `#[serde(default)]` the neighbouring flag structs
   use, defaulting **off** (D15). An existing `settings.json` is missing the key, so an
   upgrade is silent.

2. `Settings.radio-enabled` on the Slint global, plus its changed callback, wired through
   `settings_bind`'s apply-then-persist shape. `settings.slint` imports nothing, so the
   property is declared there and read wherever it is needed.

3. **The guard that matters**: an early return in `library::radio` covering every
   directory call and every logo download. Nothing outside that facade may name the Radio
   Browser client (D14), which is what makes the guard one place and provable by grep.

4. The two UI gates: `if Settings.radio-enabled` around the `SidebarItem` and the same term
   on the index-10 `ViewTransition` branch — and **nothing** around the `SectionActiveGate`
   mount. It carries a `changed` tracker and a dropped tracker-bearing branch panics; with
   the row gone the index is unreachable, so it simply never fires true. The placeholder
   fall-through keeps its plain `!= 10` term, which means a disabled build standing on 10
   would paint **nothing** rather than a placeholder — steps 5 and 6 are what close both
   ways of being there, and that is the reason they are not optional.

5. Turning it off, three consequences beyond the row: stop a station that is playing, move
   `Nav.selected-index` to My Library if the user is standing on 10, and persist that move
   through the usual `Nav.persist-selected-index` path.

6. Booting with it off: fold a persisted `last_nav_index` of 10 onto My Library, a sibling
   of `fold_retired_nav_index` rather than an extension of it. The two answer different
   questions (retired index, disabled feature) and `boot::ui_setup` composes them.

7. `melodia-ui/ui/views/settings/radio-section.slint` on the Services tab, a third card
   beside Scrobbling and Discord, with one more term in the page's `has-matches`. The
   Services page's own doc comment says "the outside accounts Melodia talks to" and needs
   widening to services, radio having no account.
   - Master row: what it does, and that it contacts radio-browser.info. Naming the service
     in the row is the point; a user deciding to stay local is entitled to know who would
     be contacted.
   - Sub-rows under `if Settings.radio-enabled`, never `visible: false` (slint#7377):
     hide HLS stations (D13), and whether to send the directory a click on play. The click
     is what keeps popularity ordering meaningful for everyone, so it defaults on and is
     opt-out rather than opt-in.
   - Every row registers its haystack, or Settings search cannot find it.

8. Turning it off leaves `radio_stations` alone. Hiding a feature is not deleting the
   user's favorites, and re-enabling restores them. A "clear station data" action beside
   the toggle is a reasonable later addition, not a requirement here.

**Gates.** Same three.

**Done when.** The toggle appears in Settings and in Settings search, flipping it adds and
removes the sidebar row with no restart, disabling while a station plays stops it and
moves you off the page, a boot with it off and a persisted nav index of 10 lands on My
Library, and with it off nothing in the app makes a request to radio-browser.info.

---

## Phase 6: Browse

**Goal.** Find any station in the world.

1. Default view with no query: top stations by click count, which is the directory's own
   answer to "what do people actually listen to".
2. Search by name, debounced, through the shared header box.
3. Facet filters as a chip strip: country, language, tag, codec, minimum bitrate, and the
   HLS toggle from D13. Facet lists come from Phase 2's cached endpoints.
4. Paging: `offset`/`limit` with a load-more at the list end. `has-more` on the global.
5. Station cards: logo, name, tags, country, bitrate and codec, a favorite toggle, a
   homepage link through `ui::launcher` (`open::that_detached`, never `open::that`), and
   the unplayable badge for HLS.
6. Logos: download through the shared client into `media::artwork::store_image`, the way
   `services::artist_images` does. **The Deezer path's domain allowlist does not
   transfer**, these hosts being arbitrary, so the guards are HTTPS, a content-length cap
   checked before the body, a content-type check, and the shared `decode_capped` bound.
   Failures are silent and fall back to the Material Symbols glyph.
7. Empty, loading and error states, including "the directory is unreachable", which is a
   normal condition rather than a bug.
8. Counts hold `UNFETCHED_COUNT` until fetched and are rewound by the section leave and by
   a tab pick, and are written above any signature guard.

**Gates.** Same three.

**Done when.** A station in a country you have never visited is two interactions away, and
leaving and re-entering the section does not refetch what is already on screen.

---

## Phase 7: Favorites and custom stations

**Goal.** The stations a user keeps.

1. Favorite toggle from either tab, writing through `library::radio`.
2. Add by URL: a `Dialog` body with URL and optional name, validated (scheme, reachable,
   resolves to audio) before the row is written. The dialog routes through the existing
   `kind` + `target-id` dispatcher, so it is one branch and one opener.

   That validation probe already parses the ICY response headers, so **let the station name
   itself when the user leaves the name blank**: `IcyHeaders` carries `name`, `genre`,
   `description` and `logo_url`, which is most of a directory row for free and is the one
   place a custom station can be as complete as a browsed one.
3. Edit and remove, with the destructive confirm the tree already uses.
4. Local filter through `row_match`, sharing the header box per Phase 4's routing.
5. Sort: name, recently added, most played, last played.
6. Recently played stations, off `last_played`, as the Favorites tab's second section or
   its own sort. Prefer a sort; a third tab contradicts D5.
7. Import and export as M3U/PLS, reusing `library::playlist_files`. Cheap here and it is
   how users move a station list between apps.

**Gates.** Same three.

**Done when.** A user-typed URL survives a restart, plays, and can be exported and
re-imported.

---

## Phase 8: The station detail

**Goal.** A station card drills into a page, and the band morphs into that station's hero.
Everything before this phase works without it, so it is the one phase that can be cut or
postponed without stranding the rest.

1. `melodia-ui/ui/views/radio/station-detail.slint`: the body under the morphed band. The
   band itself needs no new component, only its hero inputs populated.

2. The band's hero half: `detail-open`, `title` (station name), `subtitle` (country and
   language, or the homepage host), `artwork-path` and `cover`, `fallback-icon: "radio"`,
   and the `blur-a`/`blur-b`/`use-a`/`has-blur` pair from `ui::detail_artwork`. Three
   invariants come with it, all in the checklist: `hero-t` is written from
   `changed detail-open` and only seeded by its binding; the teardown rides
   `hero-collapsed()` rather than the id going away; and the chip strip stays mounted and
   fades by brush alpha rather than hanging off an `if`.

3. The source-size floor from D7: below a minimum stored dimension, skip the image tier
   and take the `fallback-icon` + `tile-bg` path. `HeroBackdrop`'s blur seeds and palette
   come off the same image, so this is one gate answering both.

4. Content: the chip strip carries tags, country, state, language, codec, bitrate and
   votes, through `HeroChipStrip` like every other hero. The body carries the actions
   (play, favorite, vote, copy stream URL, open homepage, and edit or remove for a custom
   station), the directory's last-checked state, and the session song history from this
   station.

5. Open and close: a card's click builds the open call from the whole
   `RadioStationRow` it already has (D6), the navigation rides `PendingNav` into the
   `on_applied` hook so id and navigation land in one tick, and `origin-nav-index` is
   stamped `-1` so the back arrow means "close this detail" (D5). `last_detail_ids` is
   written only when `id != 0`, and the boot seed reads it back through the same guard.

6. The filter box gains its third destination: with a detail open it filters the song
   history, which is the only list on screen. The Phase 4 dispatch function grows one arm.

7. `view_id::RADIO_DETAIL`, and a `detail-scope-changed()` re-seat on open so a re-open
   with the same id still reseats the box.

**Gates.** Same three.

**Done when.** A drill-in morphs the band rather than routing, the back arrow closes it,
a favorited station's detail survives a restart, a directory-only one lands on the tab
root instead, and a 32 px favicon produces the glyph fallback rather than a blurry tile.

---

## Phase 9: Now Playing and the OS

**Goal.** A playing station looks and behaves like a first-class source everywhere the app
already shows one.

1. Now Playing bar and full view: station logo, station name, the live title from ICY, a
   LIVE badge in place of the progress bar, elapsed listening time, and the buffering
   state, all off `PlayerVm.radio` — Phase 3's state machine leaves `current_track` `None`
   for a station, so a surface reads whichever of the two is `Some`. The transport's play
   button re-opens a paused station rather than resuming it, which Phase 3 already routes;
   what is owed here is that it doesn't *look* like a resume.
2. Seek disabled, speed disabled and pinned to 1.0 (D11), next and previous disabled (D9).
   All four are already refused by the state machine, so this is the cosmetic half.
3. MPRIS, tray and media keys: `souvlaki`'s `MediaMetadata` with `duration: None` and the
   live title as the track title. Play, pause and stop only. Phase 3 already settled the
   flicker: a buffering station keeps status `Playing`, so no arm here changes.
4. Discord presence: station name and live title, with the small-image asset the tree
   already ships. Same constraint, and settled the same way.
5. Sleep timer: the timed modes work unchanged; the end-of-track mode is disabled while a
   stream plays, since a stream has no track end.
6. Session song history: the titles seen on this station this session. **Owned here, shown
   in two places**: the Now Playing view, and the station detail from Phase 8. Cheap,
   since the ICY watch channel already carries every change, and it is the one feature the
   radio-first apps have that a library player can add for almost nothing. Capped, and
   dropped on station change.

**Gates.** Same three.

**Done when.** The tray, the media keys and the OS media popup all show the live title,
and nothing in the transport lies about what it can do.

---

## Phase 10: Hardening and documentation

**Goal.** The feature stops being new.

1. Tests, for what the UI phases add: `radio_tab` clamping, the tab-count pin against the
   `.slint` source, the hero's source-size floor as a predicate, the `id != 0` persistence
   guard, the nav fold with radio disabled, and the catalogue walk. **Plus the nav index's
   own round trip** — that `set_last_nav_index`'s clamp and `install_views`' guard both
   admit 10, which is the one failure in this feature that is silent at both ends and was
   the state of the tree until Phase 4. The band's own
   invariants are already pinned by `ui::library_tab_band_tests`; this page adds a mount,
   not a copy. Phases 2 and 3 carry their own (the directory model, the state machine's
   radio arm, the reconnect backoff, the prebuffer's starvation and frame alignment,
   playlist resolution), so what is left here is the surface.
   **One of these is a corpus walk, not a unit test**: that nothing outside
   `library::radio` names the Radio Browser client, which is what holds D15's guard to one
   place. `ui::file_dialog::tests` is the shape to copy, and
   `player::stream_source::tests::nothing_reaches_the_convenience_constructors` is the
   worked example this feature already has.
2. Memory: `/usr/bin/time -v target/release/Melodia` with a station playing, against the
   usual ceiling. The two buffers from Phase 3 are the only new resident cost and both are
   bounded by construction.
3. Error paths swept: dead station, wrong codec, HTTP 404, redirect loop, TLS failure,
   directory unreachable, and a favicon that is a 20 MB PNG.
4. `.claude/rules/radio.md`: the contract, path-scoped to the Rust slice, the `.slint`
   tree and the migration. It earns its place by spanning trees that no single file
   reaches. Everything expressible at an anchor stays in the doc comment on the constant
   or function it constrains.
5. Root `CLAUDE.md`: the nav map gains 10, the globals count and the
   `SectionActiveGate` mount count both move, the rules table gains a row, and the module
   map gains the three new homes.
6. `README.md`: the feature, that it is off until switched on, and that HLS stations are
   listed but not playable.
7. `src/player/CLAUDE.md`: the live-source arm, D8's ring and why it exists, and the
   reconnect contract.
8. **The shipped product description**, which currently promises no streaming and is now
   only true by default. `Cargo.toml`'s `[package.metadata.deb] extended-description` and
   `packaging/com.github.kenansalar.melodia.metainfo.xml` both carry it, and both are
   already published to distro repositories. The honest wording is local-first with an
   optional radio section, not silence.

**Gates.** All three, plus a release build once.

---

## Deferred, with reasons

- **HLS stations.** Symphonia has no MPEG-TS demuxer, so segments would need a demuxer
  written or vendored before a single station played. Segment playlists carrying bare
  ADTS AAC would work with a much smaller HLS client, so a partial implementation is
  possible later; D13 keeps the door open by labelling rather than hiding.
- **Recording.** Shortwave and RadioDroid both have it and it is genuinely useful, but it
  needs a segmentation heuristic over ICY title changes, an output encoder, a storage
  policy and a legal note. It is its own issue.
- **Radio scrobbling.** `services::scrobble::detector` is keyed on `track_id: i64` end to
  end, so radio scrobbling means widening `Effect` to carry artist and title strings, and
  then trusting a `StreamTitle` that stations format however they like. Worth doing, worth
  doing separately.
- **Chromecast and alarm clock.** Radio-first app features. Neither has a home in a
  library player's shape.
- **Opus streams.** Blocked on #35 and on rodio 0.23, and unblocked by them for free.

---

## Open questions

Answer before Phase 6, not before Phase 1.

1. **Does Browse default to global top stations, or to the user's country?** Country is a
   better first screen and needs a locale-to-country guess that can be wrong. Global is
   honest and less useful. Prior art splits: Shortwave defaults to a curated global list,
   RadioDroid to local.
2. **Does the favorite toggle also vote?** The API separates the two and voting is a
   deliberate act. Recommendation: no, with an explicit vote action on the station detail.
3. **Is the station history a sort of the Favorites tab or its own section within it?**
   D5 rules out a third tab; both remaining options fit.
4. **Does a card click play the station or open its detail?** Every other card in the tree
   drills in and plays from a hover control, so consistency says drill. Radio-first apps
   split: Shortwave opens a detail, Tuner plays. Recommendation: drill on click, play on
   the card's own play control, which is what the entity cards already do.

---

## References

- [Radio Browser API docs](https://docs.radio-browser.info/)
- [Radio Browser mirror list](https://all.api.radio-browser.info/json/servers) (one entry
  as of 2026-08-20, listed twice for IPv4 and IPv6)
- [stream-download](https://docs.rs/stream-download/) and [icy-metadata](https://docs.rs/icy-metadata/),
  same author. The composition in Phase 3 was read off the crate sources
  (`stream-download-0.24.3/src/{lib.rs,http/mod.rs,http/reqwest_client.rs}`,
  `icy-metadata-0.6.0/src/{headers.rs,reader.rs}`), not off the docs pages.
- `with_byte_len` setting `is_seekable`: `rodio-0.22.2/src/decoder/builder.rs:194`
- [Shortwave](https://apps.gnome.org/Shortwave/), [RadioDroid](https://f-droid.org/en/packages/net.programmierecke.radiodroid2/), [Strawberry](https://www.strawberrymusicplayer.org/)
- rodio's pull model: `rodio-0.22.2/src/stream.rs`, `init_stream`
