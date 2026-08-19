# Internet Radio

Working doc for [#29](https://github.com/KenanSalar/Melodia/issues/29). Delete when the feature ships.

Upstream facts verified **2026-08-20**. Anything marked ⚠️ **re-verify** is expected to
drift; check it on the day rather than trusting this doc.

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
- **radio-browser.info.** Roughly 50,000 stations, community-maintained, no API key, no
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
returns the same mirror list over HTTPS, so the client picks one at random per session
and falls back to a hard-coded mirror if that call fails. No new dependency, and the
random pick is what spreads load across mirrors the way SRV is meant to.

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
grows to five columns.** `queries::artwork::REFERENCE_COLUMNS` is four today and
`tasks::artwork_sweep` deletes any stored file no column names. Adding
`radio_stations.artwork_path` to that ledger is not optional; miss it and every station
logo is deleted on the next sweep.

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
| `migrations/2026XXXX_radio_stations.sql` | The one table. Branch-local until merge, so fold changes into it rather than adding a second |
| `src/entities/radio.rs` | `RadioStation`, `RadioStationRow`, `DirectoryStation`, the boundary DTOs |
| `src/database/queries/radio.rs` | Every SQL statement touching `radio_stations` |
| `src/library/radio.rs` | The facade: favorites, custom stations, history, and the directory calls the UI needs |
| `src/services/radio_browser/mod.rs` | Mirror discovery, the shared client's request builder, error mapping |
| `src/services/radio_browser/model.rs` | The API's JSON shapes, deserialized once |
| `src/services/radio_browser/query.rs` | Search and facet-list request construction, paging |
| `src/player/stream_source.rs` | Opening a URL into a decodable reader: stream-download, ICY, playlist resolution |
| `src/player/prebuffer.rs` | The ring buffer and its feed thread (D8) |
| `src/ui/radio/mod.rs` | Slice install, section-active hook, row and cover plumbing |
| `src/ui/radio/detail.rs` | The station detail: open, close, hero artwork, the restart seed |
| `src/ui/radio/callbacks/*.rs` | Browse, Favorites, detail and playback wiring |
| `melodia-ui/ui/globals/radio.slint` | The `Radio` global |
| `melodia-ui/ui/views/radio-view.slint` | Page chrome only: the band, tab routing, scroll body, overlay scrollbar |
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
| `melodia-ui/ui/layout/sidebar.slint` | One `SidebarItem`, after My Library |
| `melodia-ui/ui/app-window.slint` | The `export { }` name list, one `ViewTransition` branch, two `SectionActiveGate` mounts |
| `melodia-ui/ui/models.slint` | `RadioStationRow`, plus the live fields on `PlayerVm` |
| `melodia-ui/ui/settings.slint` | `radio-enabled` and its changed callback on the `Settings` global |
| `melodia-ui/ui/views/settings/pages/services-page.slint` | A third card, and one more term in `has-matches` |
| `src/services/settings/data.rs` | `RadioFlags`, flattened into `SettingsData`, defaulting off |
| `src/boot/ui_setup.rs` | Folding a persisted nav index of 10 when radio is off |
| `src/services/view_state.rs` | `radio_tab: i32` |
| `src/test_support.rs` | `CALLBACK_HOMES` grows to 13 |
| `src/database/queries/artwork.rs` | The fifth reference column (D12) |
| `src/player/state.rs` | The radio arm of the state machine |
| `src/player/rodio_backend.rs` | `play_stream`, and `build_source` made generic over its reader |
| `src/player/handlers.rs` | The monitor's live-source arm |
| `scripts/icons.txt` | `radio` and whatever else the cards use |
| `melodia-ui/translations/*/LC_MESSAGES/melodia-ui.po` | Every new msgid, in all six |

### What must not grow

- **`src/player/rodio_backend.rs` does not learn HTTP.** It takes a reader that is already
  open. Everything network-shaped lives in `stream_source.rs`.
- **No second `decode_file`.** `build_source` becomes generic over `R: Read + Seek + Send + Sync`
  and both callers hand it what they have. A parallel `build_stream_source` is the
  duplication this note exists to prevent.
- **No second reference-column ledger.** D12 is one array, not a query that also lists them.
- **No second "does this row match" predicate.** The Favorites filter goes through
  `src/ui/row_match.rs` like the other sixteen surfaces.
- **No second tab bar, and no third band.** `LibraryTabBand` is mounted, not copied, and
  not parameterised into a shared ancestor with `MosaicTabHero`. The two are siblings for
  reasons `ui-patterns.md` argues, and `ui::library_tab_band_tests` holds them to it.
- **No second HTTP client.** `services::build_http_client`'s pool is process-wide and its
  User-Agent is what the directory asks for.

---

## Cross-cutting checklist

The things that are silent when missed. Each is checked off in the phase that owns it.

- [ ] `radio_stations.artwork_path` in `queries::artwork::REFERENCE_COLUMNS` **and** its
      union query (D12).
- [ ] Two `SectionActiveGate` mounts at `index: 10`, one per tab, with `tab-index` and
      `current-tab`. A tab leave has to be the same event as a section leave.
- [ ] The persisted nav index is written **before** `wire_all`, so the slice's
      `section_active` shadow seeds correctly. Same for `seed_tab`.
- [ ] `radio_tab` clamped on read through `ui::tab_bar::clamp_tab` against
      `Radio.tab-count`, never a Rust const.
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
- [ ] The two `SectionActiveGate` mounts stay mounted when radio is disabled. They carry
      `changed` trackers, and dropping a tracker-bearing branch panics.
- [ ] The Settings sub-rows sit under `if Settings.radio-enabled`, never `visible: false`
      (slint#7377), matching the crossfade rows.
- [ ] The new Settings rows register their haystacks, or the page's search cannot find them.
- [ ] Every new `@tr` msgid lands in all six catalogs. The catalogue walk test fails
      otherwise, and a miss ships silently as the English msgid.
- [ ] Every new icon name is in `scripts/icons.txt`, the fonts are re-subset, and
      `scripts/check-icons.py` passes. A missing name renders as tofu.
- [ ] `cargo tree -i native-tls` is empty after the dependency add. `stream-download`'s
      default features pull `reqwest/default-tls`, which is OpenSSL on Linux and changes
      what `cargo-deb`'s `$auto` resolves to.
- [ ] No `unwrap()`, no `#[allow(dead_code)]`, no `sed`-driven edits.
- [ ] Thread names stay under 15 bytes.
- [ ] Nothing logs a stream URL that carries a token in its query string.

---

## Phase 1: The table and the facade

**Goal.** Stations exist in the database and `library::radio` answers questions about
them. No UI, no network, no audio.

1. Write `migrations/2026XXXX_radio_stations.sql`:

   ```sql
   CREATE TABLE radio_stations (
       id             INTEGER PRIMARY KEY AUTOINCREMENT,
       -- NULL for a user-added URL; the directory's uuid otherwise, so a
       -- re-import updates rather than duplicates.
       station_uuid   TEXT UNIQUE,
       name           TEXT    NOT NULL,
       stream_url     TEXT    NOT NULL,
       homepage       TEXT,
       favicon_url    TEXT,
       artwork_path   TEXT,
       tags           TEXT    NOT NULL DEFAULT '',
       country_code   TEXT    NOT NULL DEFAULT '',
       language       TEXT    NOT NULL DEFAULT '',
       codec          TEXT    NOT NULL DEFAULT '',
       bitrate        INTEGER NOT NULL DEFAULT 0,
       is_favorite    INTEGER NOT NULL DEFAULT 0,
       sort_key       TEXT    NOT NULL DEFAULT '',
       date_added     TEXT    NOT NULL,
       last_played    TEXT,
       play_count     INTEGER NOT NULL DEFAULT 0
   );
   CREATE INDEX idx_radio_stations_last_played
       ON radio_stations(last_played DESC) WHERE last_played IS NOT NULL;
   CREATE INDEX idx_radio_stations_favorite
       ON radio_stations(is_favorite) WHERE is_favorite = 1;
   ```

   `sort_key` is the `natord` column the tracks table already carries, so favorites sort
   the way every other name column in the app sorts.

2. `src/entities/radio.rs`: `RadioStation` (the row), `RadioStationRow` (the boundary DTO
   mirroring the Slint struct exactly), and a `From` between them.

3. `src/database/queries/radio.rs`: upsert by `station_uuid`, insert custom, list
   favorites, list recent, set favorite, delete, bump play count and stamp `last_played`,
   set `artwork_path`.

4. `src/library/radio.rs`: the async facade over the above. Directory functions land in
   Phase 2 and share this module, so the UI has one import either way.

5. Add the fifth artwork reference column (D12) with its test.

**Gates.** `cargo clippy --all-targets --locked -p Melodia -- -D warnings`,
`cargo fmt --all --check`, `cargo test`. A migration dry run through
`/usr/bin/sqlite3` before the compile cycle is the cheap way to catch a typo.

**Done when.** Unit tests over `DbPool::test_pool()` cover the upsert-by-uuid path
(re-adding a directory station updates rather than duplicates), the favorite toggle, and
the artwork reference set including the new column.

---

## Phase 2: The directory client

**Goal.** `library::radio::search(...)` returns stations from radio-browser.info. Still no
UI and no audio.

1. `src/services/radio_browser/mod.rs`:
   - Mirror discovery per D1: one `GET /json/servers` against
     `https://all.api.radio-browser.info`, random pick, cached in a `OnceLock` for the
     session, hard-coded fallback on failure.
   - All requests through `services::build_http_client`. Its ten-second connect timeout
     and per-read deadline are already what this wants.
   - Errors map to `AppError::network(msg, source)`, never a flattened `format!("{e}")`.

2. `model.rs`: the station JSON, deserialized with `#[serde(default)]` throughout. The API
   sends `null` for several fields and adds new ones without notice. Fields worth
   carrying: `stationuuid`, `name`, `url_resolved` (prefer over `url`), `homepage`,
   `favicon`, `tags`, `country`, `countrycode`, `language`, `codec`, `bitrate`, `hls`,
   `votes`, `clickcount`, `lastcheckok`.

3. `query.rs`: `/json/stations/search` with `name`, `country`, `countrycode`, `language`,
   `tag`, `codec`, `bitratemin`, `order`, `reverse`, `offset`, `limit`, `hidebroken=true`.
   Plus `/json/stations/topvote` and `/json/stations/topclick` for the empty-query
   default, and `/json/countries`, `/json/languages`, `/json/tags` for the facet lists.

4. Click and vote: `/json/url/{uuid}` is what the API asks be called on every play, and it
   is what keeps the popularity ordering meaningful for everyone. `/json/vote/{uuid}`
   backs an explicit user action only.

5. Facet lists are cached in memory for the session. They are large and near-static.

**Gates.** Same three. Tests are offline: deserialization against captured fixtures,
query-string construction, mirror fallback. No test hits the network.

**Done when.** A throwaway `#[ignore]`d test or a scratch binary prints real search
results, and `cargo tree -i native-tls` is still empty.

---

## Phase 3: Stream playback

The load-bearing phase. Everything here is argued once and then stays put.

**Goal.** A URL plays through the existing DSP chain, with buffering and reconnect.

1. **Dependencies.** ⚠️ **re-verify** versions with `cargo search` on the day:

   ```toml
   # Read+Seek over an HTTP stream, with a circular buffer for infinite ones. Same
   # author as icy-metadata below and designed to compose with it.
   stream-download = { version = "0.24.3", default-features = false, features = [
       "http", "reqwest", "reqwest-rustls",
   ] }
   # Icecast/SHOUTcast in-band metadata: the Icy-MetaData request header, the
   # icy-* response headers, and the reader that strips the metadata blocks back out.
   icy-metadata = { version = "0.6.0", features = ["reqwest"] }
   ```

   `default-features = false` is the load-bearing half. The defaults enable
   `reqwest`'s own defaults, which means `default-tls`, which is OpenSSL on Linux and
   would land in `cargo-deb`'s `$auto` dependency set and in every CI toolchain.
   `stream-download` requires `reqwest ^0.13.4`, which unifies with the pinned 0.13.4
   rather than resolving a second copy.

2. **`src/player/stream_source.rs`.** Opening a URL, in order:
   - Resolve playlist indirection. A user-typed URL may point at `.pls`, `.m3u`,
     `.m3u8` or `.asx` rather than at audio. Directory stations arrive pre-resolved in
     `url_resolved`, so this runs for custom stations and as a fallback. Cap the follow
     depth at one and the body size, then take the first entry.
   - `RequestIcyMetadata` sets the `Icy-MetaData: 1` request header.
   - `StreamDownload::new_http` over a bounded circular buffer, prefetch sized from the
     advertised bitrate.
   - `IcyMetadataReader` wraps that, its callback publishing `StreamTitle` on a
     `tokio::sync::watch`. The callback runs on the feed thread, so it must not block:
     a `watch` send is the whole handler.
   - `Decoder::builder().with_data(reader).with_seekable(false)`, hint from the codec the
     directory reported. Note `with_byte_len` also sets `is_seekable`, so it must not be
     called here.

3. **`src/player/prebuffer.rs`.** D8's ring:
   - A feed thread pulls the decoder into a bounded SPSC ring of `f32`.
   - `Source::next()` pops without blocking and returns silence when starved. The fill
     must stay frame-aligned; `EqSource`'s `frame_phase == 0` poll gate and the mixer's
     channel parity both depend on whole frames.
   - Starvation is published as an `AtomicBool` the view model reads, which is the
     buffering indicator rather than a second mechanism for it.
   - Sizing is a budget decision: a few seconds of stereo float, plus the circular
     download buffer, is the resident cost of a playing station and it is bounded by
     construction. Measure it against the usual ceiling at Phase 10.
   - Thread name `radio-buffer`, twelve bytes, inside the kernel's fifteen.

4. **Backend.** `build_source` becomes generic over `R: Read + Seek + Send + Sync + 'static`
   so both the file and the stream path share it. Add `RodioPlayer::play_stream`, and
   stage the opened reader the way `preload_gapless` stages its source: the async task
   calls `stage_stream(prepared)` before emitting, and the `PlayerAction::PlayStream`
   arm takes it out under the deck lock. `PlayerAction` derives `Clone` and `PartialEq`
   and must stay plain data, which is why the reader does not ride on it.

5. **State machine.** A `radio: Option<RadioNowPlaying>` arm on `PlayerState` carrying
   station id, name, artwork path and the live title. `PlaybackStatus::Loading` already
   exists and is currently unused; it becomes the connecting and buffering state.
   `has_next`/`has_previous` are false while it is `Some` (D9), duration is 0, position is
   elapsed listening time.

6. **Monitor.** `player/handlers.rs` gets a live-source arm ahead of the gapless and
   crossfade gates, both of which are meaningless without a track end. End of stream means
   the connection dropped, so it triggers reconnect rather than `advance_skip`.

7. **Reconnect.** Bounded exponential backoff from about a second, a small attempt cap,
   status `Loading` throughout, and a toast on final give-up through `services::toast`.
   The playing station is remembered across the retry so the UI does not blank.

8. **Guards.** `start_or_skip`'s `Path::exists()` pre-flight must not run for a URL.

**Gates.** Same three, plus `cargo build` once. Do not launch the app; the manual test
gate is the user's.

**Done when.** A hard-coded URL plays end to end, the visualizer moves, the equalizer
bites, pulling the network cable buffers and then reconnects, and no cpal underrun is
logged when it does.

---

## Phase 4: The section shell

**Goal.** The sidebar row, the page, the two tabs and the persistence. Empty tabs are
fine; they fill in Phases 6 and 7, and the band's hero half stays idle until Phase 8.

1. `melodia-ui/ui/globals/radio.slint`: the `Radio` global. `tab-browse: 0`,
   `tab-favorites: 1`, `tab-count: 2` as the sole definition, `tab-idx`, `tab-changed`,
   the two row models, the two counts, the section hook, `request-cover`, and the action
   callbacks. Imports nothing from siblings, so the global DAG stays shallow.

2. `melodia-ui/ui/models.slint`: `RadioStationRow`. No `cover_img` field; the thumbnail
   resolves per instantiated row through `request-cover(artwork_path, generation)` like
   every other grid row.

3. `melodia-ui/ui/views/radio-view.slint`: page chrome only, modelled on
   `my-library-view.slint`. `LibraryTabBand` with two labels and two icons, the inline
   `@tr` array in tab order, the `page-w` mirror written from `changed width`, the 1 ms
   mount `Timer` that re-runs it, the seed at the row's own floor, and a `ScrollView` with
   both policies off under a root `OverlayScrollbar`. The band's hero inputs are wired but
   idle: `detail-open: false` and nothing else populated.

4. `app-window.slint`: the export list, one `ViewTransition` branch at index 10, and two
   `SectionActiveGate` mounts at `index: 10` differing only by `tab-index`. A drill-in does
   not move `tab-idx`, so the detail needs no third gate.

5. `sidebar.slint`: `SidebarItem { index: 10; label: @tr("Radio"); icon: "radio"; }`
   directly after My Library.

6. `src/ui/radio/`: the slice, its `install` taking `ViewCtx`, the section hook, and the
   tab persistence through `radio_tab` in `views.json`, clamped on read.

7. `CALLBACK_HOMES` grows to 13. `scripts/icons.txt` gains `radio`. Six catalogs gain the
   new msgids.

**Filter box routing, stated once here because it is the one thing this page does that the
other tabbed pages do not.** The band carries a single box, and the two tabs want
different things from it: on Browse it is a directory query, debounced and sent over the
network; on Favorites it is a local `row_match` filter. One dispatch site in Rust routes a
settled keystroke by the live tab, the way `my_library/filter.rs::dispatch` routes its
nine surfaces. The placeholder text changes with the tab so the difference is visible.
Phase 8 adds a third destination for the same box, which is why the dispatch is a function
from the outset rather than an `if` that grows.

**Gates.** Same three. `scripts/check-icons.py` after re-subsetting.

**Done when.** The row appears under My Library, both tabs mount, the tab survives a
restart, and the section gates fire on tab and nav moves.

The sidebar row and the router branch land here **ungated**, and Phase 5 adds the `if`
term to each. That is three one-line edits of churn, taken deliberately: the alternative
is a phase whose only deliverable is a toggle with nothing yet to toggle, which cannot be
tested by hand.

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

4. The three UI gates: `if Settings.radio-enabled` around the `SidebarItem`, the same term
   on the index-10 `ViewTransition` branch, and nothing around the two
   `SectionActiveGate` mounts. Those carry `changed` trackers and a dropped
   tracker-bearing branch panics; with the row gone the index is unreachable, so they
   simply never fire true.

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
   state from Phase 3's atomic.
2. Seek disabled, speed disabled and pinned to 1.0 (D11), next and previous disabled (D9).
3. MPRIS, tray and media keys: `souvlaki`'s `MediaMetadata` with `duration: None` and the
   live title as the track title. Play, pause and stop only.
4. Discord presence: station name and live title, with the small-image asset the tree
   already ships.
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

1. Tests: the state machine's radio arm, the reconnect backoff as a pure function, the
   prebuffer's starvation and frame alignment, playlist resolution, the directory model's
   deserialization, `radio_tab` clamping, the tab-count pin against the `.slint` source,
   the hero's source-size floor as a predicate, the `id != 0` persistence guard, the nav
   fold with radio disabled, and the catalogue walk. The band's own invariants are already
   pinned by `ui::library_tab_band_tests`; this page adds a mount, not a copy.
   **One of these is a corpus walk, not a unit test**: that nothing outside
   `library::radio` names the Radio Browser client, which is what holds D15's guard to one
   place. `ui::file_dialog::tests` is the shape to copy.
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
- [Radio Browser mirror list](https://all.api.radio-browser.info/json/servers)
- [stream-download](https://docs.rs/stream-download/) and [icy-metadata](https://docs.rs/icy-metadata/)
- [Shortwave](https://apps.gnome.org/Shortwave/), [RadioDroid](https://f-droid.org/en/packages/net.programmierecke.radiodroid2/), [Strawberry](https://www.strawberrymusicplayer.org/)
- rodio's pull model: `rodio-0.22.2/src/stream.rs`, `init_stream`
