# Splitting `src/` into a workspace

Working doc. Validation of the issue, the dependency graph as the code actually has it, and the
order the cuts come out in. Harvest into `docs/adr/` when
[#84](https://github.com/KenanSalar/Melodia/issues/84) ships, not before: the boundary rationale
below is exactly what #84 exists to stop evaporating.

Status: **not started** · Issue:
[#83](https://github.com/KenanSalar/Melodia/issues/83) · Created: 2026-09-03 · Validated against
`93b47dfa`

> The issue body carries the argument for *why* a workspace. This doc carries what a read of the
> tree found that the body does not, and it is the source [#84](https://github.com/KenanSalar/Melodia/issues/84)
> will draw on, so the rationale for each boundary belongs here rather than in the issue thread.
> Where this doc and the code disagree, the code is right.

## Prerequisites

- [x] Radio ships.
- [x] #79 ships, `PlaybackSource` included. Closed 2026-09-01; `src/player/CLAUDE.md` documents
      `source_allows(PlaybackSource::advances_queue)` and the five surviving `is_radio` sites.
- [ ] `cargo build --timings` on a clean target, kept, so the extraction phases can be judged
      rather than assumed.

## What validation changed

All five cycles in the issue are real code edges. Nine of its numbers are not, four of its
dependency edges are missing, and two of its crate boundaries are wrong. The counts decide which
cut is cheapest; the edges decide the shape of the graph.

| issue or earlier draft says | tree says |
|---|---|
| `Track::replaygain()` | `TrackSummary::replaygain()`, `entities/track.rs:73-82` |
| `state/mod.rs:133,137`, 2 sites | `:151,155,277,278,280`, 5 code lines |
| `ui` reaches `state` at 135 | 138, all production |
| `ui` reaches `tasks` at 27 | 29, and 25 of them are one function |
| `player` reaches `services` at ~6 | 13 lines, 12 of them code; `player/hls/` postdates the issue entirely |
| `ui` reaches `media` at 34, 26 `cover_thumbs` | 28 non-test, 22 of them `cover_thumbs` |
| ~134 of 225 `.claude/rules` globs break | **83 of 165**, and 134 of 165 once `melodia-ui`, `tests/` and `build.rs` move too |
| 21 test files anchor on a tree root | **28**: 27 through the seven constants, plus `library/tests/radio_tests.rs:17` direct |
| 241 relative `include_str!` across 46 files | **243 across 47 files** (254 literal-arg sites across 48, but the 11 without a `../` are `minisign_tests.rs`'s crate-local `fixtures/`); the 135 reaching `melodia-ui/ui/` is exact |
| ten non-binary `CARGO_PKG_VERSION` sites | 10 expansions, **9 non-binary, 7 non-test**, and `ui/callbacks/updater/install.rs:83` is missing from the list |
| `src/` is 81,131 production lines | **86,820**, of 133,772 total. The table below is remeasured |

Four edges the issue's table omits, each a real call and not a doc link:

- `media/metadata.rs:248` calls `player::file_decode::probe_duration`, so **store depends on audio**.
  It is the sole edge `media/` has into that directory, which `src/player/CLAUDE.md` already says.
- `station_logo.rs:140` and `deezer.rs:245` call `artwork::store_image`, so **net depends on artwork**.
- `scanner.rs:52` and `metadata.rs:178` take `&artwork::CoverCache`, so **store depends on artwork**.
- `services/artist_images.rs:9-12` names `database::{DbPool, queries}` *and* `media::deezer`, so a
  "net owns everything that opens a socket" rule would put **store on net's dependency line**. It is
  an orchestrator rather than a fetcher and belongs in app; see finding 3.

Four files outside `src/ui/` name `crate::AppWindow`, not the two the issue lists: `themes/apply.rs:8`,
`tasks/updater_daily.rs:47`, `tasks/rss_sampler.rs:47` and `services/dwm_titlebar.rs:24`
(Windows-gated). Two of the four survive the plan below: `updater_daily`, which lands in app and may
name it, and `dwm_titlebar`, which lands in platform and may not, so B8 narrows it.

And the three-way `media/` split has a hole: `mod.rs` declares 14 modules and the issue's tiers name
13. `rating_tags.rs` is unassigned and belongs with ingest. Going the other way,
`services/material_you.rs` is an image file misfiled under services (`slint`, `image_decode::decode_capped`,
`themes`) and should join the image tier. `services/radio_blocklist/` is unassigned in both documents;
finding 5 places it.

### Sizes, remeasured

`src/` totals 133,772 lines, 46,952 of them tests, so **86,820 production**. Per directory,
production only:

```
ui       38,243      services 12,378      player   11,734
library   6,086      database  4,649      media     3,953
tasks     3,590      entities  1,699      themes    1,379
boot        903      state       511
```

`player/` is the one that moved. The issue budgeted 7.7k for `melodia-audio` against today's 11,734,
because #79 and `player/hls/` both landed after it was written. That is a third of the reason the
audio stack is three crates below rather than one.

## Findings the issue does not carry

Ranked by how quietly they fail.

1. **`melodia-views` cannot exclude `melodia-store` as the tree stands, and that exclusion is the
   entire justification for the boundary.** Two production files break it:
   `src/ui/callbacks/tags.rs:33` imports `crate::media::tag_writer::{self, ArtworkEdit, FieldEdit,
   TagEdit}`, and `src/ui/radio/logos.rs:26` imports `crate::media::station_logo::StoredLogo`.
   Three of the four types are boundary DTOs the UI *builds* (`build_edit`, `diff_str`,
   `diff_parsed`, `diff_bpm` at `tags.rs:466-529`) before handing them to
   `library::tags::apply_tag_edit`; `StoredLogo` is two fields and appears only as the `Ok` type of
   an awaited future. The one real call is `tag_writer::read_lyrics(&path)` at `tags.rs:127`.
   Under the proposed graph the first file puts store on views' dependency line and the second puts
   net there. Both are the rule the root `CLAUDE.md` already states, so both are cheap, and both
   have to land in Phase A: until they do, the post-extraction check this whole issue is verified
   by is false on the day it is written.
2. **`melodia-testkit` as the issue draws it does not compile.** Cargo permits a dev-dependency
   cycle for *resolution*, and the resolver reference then says why that is not enough: a test
   binary may link two distinct copies of the same library. Building `melodia-store`'s unit-test
   target compiles `melodia-store` a second time under `cfg(test)`, so a `DbPool` handed back by a
   testkit that depends on the rlib is a different type from the one the test's own crate names.
   That is the `expected slint::Weak, found slint::Weak` wall the root `CLAUDE.md` already
   documents, with a different type name. The fix is nearly free: of the ~35 items in
   `src/test_support.rs` exactly two touch workspace types, `paths_in()` (`:447`, returns
   `config::Paths`, 2 call sites) and `resolved_home()` (`:600`, calls `services::home_dir_string`,
   1 call site). **`melodia-testkit` is a leaf with no workspace dependency at all**, those two move
   to the crates owning their types, and nothing needs the cycle exemption.
3. **`version = "0.0.0"` on internal crates would ship a bug.** Seven non-binary production sites read
   `env!("CARGO_PKG_VERSION")`: the updater's compare (`tasks/updater_daily.rs:137`,
   `ui/callbacks/updater/check.rs:33`, `ui/callbacks/updater/install.rs:83`), the Settings version
   label (`ui/settings/updater_settings.rs:39`), the HTTP user agent (`services/mod.rs:89`), crash
   reports, and ListenBrainz's submitted client version. Separately, `repository` and `homepage` sit
   on `[package]` rather than `[workspace.package]`. They resolve *today*, because the root package
   declares them as literals; they go empty in whichever crate `ui/settings/about.rs:18` lands in,
   because there is nothing in `[workspace.package]` to inherit. `melodia-ui/Cargo.toml` is the
   precedent: it sets no `repository` at all. **Every member takes `version.workspace = true`, and
   `repository`/`homepage` move into `[workspace.package]`.**
4. **The binary's artifact name is load-bearing and the issue renames it.** `Melodia` has no
   `[[bin]]` table, so the artifact is `target/release/Melodia`, spelled in `Cargo.toml:90`,
   `release-build.yml:60`, all three of `scripts/build-{appimage,tarball,rpm}.sh`, and on Windows
   `wix/main.wxs:268` (`Source`, `Name`), `:281` (`RestartResource ProcessName`) and `:319-324`
   (`HKLM\Software\Classes\Applications\Melodia.exe`). Those registry keys are what an in-place MSI
   upgrade matches on, so a rename strands the file associations of everyone already installed.
   **The bin package is `melodia` with `[[bin]] name = "Melodia"`.** One line, `-p melodia` works,
   `cargo wix --package melodia` works, and every path above is untouched.
5. **Two build-script constraints decide crate membership, and neither is a preference.**
   `cargo:rustc-env` reaches only the crate whose build script emitted it, and there are three
   `option_env!` sites: `services/scrobble/providers/lastfm.rs:24,25` and
   `services/discord/mod.rs:29`. **Scrobble and Discord therefore land in the same crate**, which
   owns `load_dotenv()`. Splitting them across net and app ships a build that is silently keyless
   and ListenBrainz-only. Likewise `OUT_DIR` is per-crate and `build.rs:22` `include!`s
   `src/services/radio_blocklist/source.rs` by path, so the bake, the `blake3` build-dependency and
   the two `.env.radio.*.local` files move with `radio_blocklist` (702 production lines) wherever it
   goes. Its consumers are `radio_browser` (net) and `library/radio/{authoring,radio_files}` (app),
   and it imports only `entities::radio`, so net is the placement. Re-root the two dotfiles at the
   repo root so the existing secret workflow keeps its paths. `build.rs`'s `read_to_string(".env")`
   is CWD-relative and cargo sets CWD to the package root: a missed hop breaks local dev only,
   because CI passes the keys as environment variables and the environment wins, which is the worst
   possible place for it to break quietly.
6. **`tests/` has no home under a virtual manifest.** `tests/{headless,crossfade,stream_rate}.rs`
   and `tests/{assets,fixtures,common}/` belong to the root package today; a virtual root has no
   package, so nothing compiles them and no job notices. `ASSETS_DIR` points into that directory
   from 6 unit-test files and `tests/headless.rs:33` joins `CARGO_MANIFEST_DIR` itself. They move to
   `crates/melodia/tests/`, which is also where the corpus walks should go; see finding 8.
7. **About half the `.claude/rules` path globs stop matching, and the pin cannot see it.**
   83 of 165 entries begin `src/` and 43 name `melodia-ui/`; another 5 name `tests/` and 3 name
   `build.rs`, which finding 6 and Phase C move as well, so the total is 134 of 165. Three more
   name `Cargo.toml` and go half-blind rather than dark: they keep matching the virtual root and
   stop reaching the thirteen member manifests, which is where the rules they carry now apply.
   `src/tests/test_support_tests.rs:89` does
   `if glob.contains('*') { continue; }`, deliberately, because a glob may legitimately describe an
   empty tree, so every `src/**/*.rs` entry goes green while matching nothing and `code-style.md`
   and `tokio.md` stop loading for all Rust in the tree. **Drop that skip in Phase A, on its own**,
   not in Phase D beside the 83 rewrites: a glob that has already rotted is otherwise
   indistinguishable from one the split broke.
8. **The tree-root anchors narrow, and the suite says so.** 28 test files anchor on a root, 10 of
   them on `SRC_DIR`. Every one of them is guarded. Three vacuity floors (`MIN_SOURCES = 200`,
   `MIN_SLINT_SOURCES = 100` and `MIN_UI_SOURCES = 180`, at `test_support.rs:29`, `:20` and `:82`)
   cover `SRC_DIR`, `UI_DIR` and `UI_SRC_DIR`, and the rest fail on an explicit assertion rather
   than passing vacuously: `test_support_tests.rs:57` asserts `RULES_DIR` lists at all and carries
   a `MIN_LITERALS` floor besides, and `services/tests/mod_tests.rs` asserts on an unreadable list
   at `:419` and on non-empty content at `:533`, `:600`, `:614` and `:702`. So a narrowed anchor is
   a red suite rather than silent rot, and **finding 7's globs are the only class that fails
   quietly**, precisely because of the `*` skip. Two things still fix this whole class at once: one
   workspace-root anchor (Phase A), and one home for every walk that asks a question of the *tree*
   rather than of a crate.
   Left where they are, `library::radio::tests::every_outbound_call_takes_its_client_from_behind_the_switch`
   and `radio_browser::tests::only_the_radio_facade_reaches_the_directory_client` end up in
   different crates, each seeing half of the property they jointly hold.
9. **Compile-time path breaks. Loud, but numerous.** 243 relative `include_str!` sites across 47
   files, 135 of them reaching into `melodia-ui/ui/`, each hop count a function of that file's depth
   from the package root. Then `sqlx::migrate!("./migrations")` at `database/mod.rs:214` and `:313`,
   and `services/updater/minisign.rs:34-35`'s `assets/updater-pubkey.b64`. Two more break at test
   runtime rather than at compile time, being `CARGO_MANIFEST_DIR` anchors a `read_dir` consumes:
   `library/tests/radio_tests.rs:17` and `ui/settings/tests/locale_tests.rs:22`.
10. **`[package.metadata.deb]`'s eight asset paths and its `license-file` are package-root-relative**,
    and `target/` stays at the virtual root. cargo-deb also special-cases a `target/release/` prefix
    when `--target` is passed, and that prefix stops matching literally once the manifest sits at
    `crates/melodia/`, so this one gets a real `cargo deb --target … --no-build` run rather than an
    assumption. Under a virtual manifest a bare `cargo build` and `cargo deb` both build every
    member, so `release-build.yml:83` and `:124` need `-p`.
11. **A member missing `[lints] workspace = true` leaves the gate with no CI signal.** It drops
    `unwrap_used`, `unsafe_code`, `await_holding_lock` and all of pedantic, and
    `cargo clippy --all-targets -- -D warnings` reports zero warnings for a crate with no lint table.
    The sibling trap is `default-members`: narrowing it to the binary would silently drop every other
    crate's *test* targets from `--all-targets`. Leave every member default and pass `--workspace`.
12. **Shared third-party dependencies have to move to `[workspace.dependencies]`, and the issue
    never says so.** `slint` is already there and the comment beside it gives the whole argument: a
    drift resolves two `slint` crates and the build dies in a wall of `expected slint::Weak, found
    slint::Weak`. The same holds for every dependency more than one member names, and the pin that
    matters most is `reqwest`'s `default-features = false`, which `.claude/rules/radio.md` calls
    load-bearing and which net and audio both need.
13. **Facade re-exports launder the dependency the split exists to forbid.** The landing tactic
    below has each crate `pub use` what it took, which is right for the migration and needs a stated
    stopping rule: **a crate must not `pub use` a type from a crate its own dependents are meant to
    be unable to reach.** `melodia-app` re-exporting a store type hands views that type back without
    views ever naming store in its manifest, and the compile error the exercise buys never fires.
14. **`toast` and `play_count_flusher` are the same primitive written twice**: a
    `OnceLock<UnboundedSender<E>>` plus a plain enum, producer half dependency-free, consumer half
    owning the I/O. `services/toast.rs:10` already notices the resemblance in prose. What must not
    be flattened along with it: `try_send` returns `bool` because callers branch on it, `notify`
    returns `()`.
15. **`player` names `DbPool` for a test fallback in one file and for real in another.** The direct
    UPDATE at `actions.rs:107-127` runs only when `try_send` returns false, and the comment there
    says exactly that: install the flusher in test contexts and `db: &DbPool` leaves
    `execute_actions` and `emit_and_execute`. But `handlers.rs:239` holds `pub db: DbPool` as a real
    field and `:441` runs `queries::track::update_last_position` in production with nothing behind
    it, so A5's third bullet is not optional. One of those two is a deletion; the other is the
    snapshot-upward move.
16. **Stale doc comment** at `tasks/rss_sampler.rs:16-21` names an `ui::window_chrome::is_queue_sheet_open`
    import that no longer exists anywhere in `src/tasks/`; the real imports are `crate::AppWindow`
    and `ui::view_tag::format_view` at `:45-49`.
17. **Nothing in the gate runs `cargo doc`.** The 162 bracketed intra-doc links that will stop
    resolving degrade silently to plain text rather than failing. Either accept that explicitly or
    add a `cargo doc` step, but do not leave it unstated.

**Survives untouched**, verified, so no work is planned for it: the four `[workspace.package]`
version scrapes (they anchor on `^\[`, so the `[package]` table vanishing is fine),
`pr-validation.yml`'s `changes` filter (a denylist rooted at `'**'`, so `crates/**` falls through
correctly), every `$REPO_ROOT`-based path in the packaging scripts, and `wix/main.wxs`'s
`$(sys.SOURCEFILEDIR)..`.

## The graph, corrected

Thirteen members plus `melodia-ui`. Four changes from the issue's nine, each resting on a seam the
code already has rather than on an abstraction invented to satisfy the split.

```
melodia-core          error, config, entities (+ the boundary DTOs), utils,
                      describe, atomic json/text writers                   -> nothing
melodia-testkit       env lock, path anchors, corpus walkers               -> nothing (dev-dep of all)
melodia-artwork       artwork/, cover_thumbs, image_decode, logo_tile,
                      material_you                                         -> core, slint
melodia-net           http primitives, 4 fetchers, radio_browser,
                      radio_blocklist + its bake, updater's net half       -> core, artwork
melodia-platform      tray, media keys, single_instance, logging,
                      crash_report, system_theme, dwm_titlebar, palette
                      derivation, updater's platform half                  -> core
melodia-audio         audio.rs vocabulary, decode, file/stream/hls
                      sources, aac                                         -> core, net
melodia-playback      output device/mixer/convert, EQ, ReplayGain,
                      crossfade, dsp, spectrum, visualizer, waveform,
                      decks, stream_health                                 -> core, audio
melodia-engine        PlayerState, queue, actions, handlers, types,
                      now_playing, event_sink, backend                     -> core, audio, playback
melodia-store         database/, scanner, metadata, watcher, tag_writer,
                      rating_tags, self_writes                             -> core, artwork, audio
melodia-integrations  scrobble, discord, and load_dotenv with them         -> core, net, engine
melodia-app           library/, tasks/, state/, settings, view_state,
                      artist_images, updater orchestration                 -> all above, melodia-ui
melodia-views         src/ui/, themes::apply                               -> app, engine, playback,
                                                                              artwork, platform,
                                                                              core, melodia-ui
melodia (bin)         main, boot/, shutdown, tests/                        -> views, app
                      [[bin]] name = "Melodia"
```

The directories nest and the crates do not. A `->` entry is a `path = "../<name>"` line in that
crate's manifest, so a crate can be named by several others and `melodia-core` is named by eleven.
That is a graph, and no tree can hold it.

Deltas from the issue: audio sits below store rather than beside it; artwork is depended on by net,
store and views rather than being their peer; the single `melodia-audio` becomes three crates;
scrobble and Discord become their own crate because `load_dotenv` forces it; `themes` splits so
platform no longer carries `melodia-ui`; and `melodia-testkit` is a leaf rather than a cycle.

### Why the audio stack is three crates and not one

The issue defers this, on "librespot splits fetch/decode from mixer/sink and that is probably where
this ends up, but not on day one. Let the code prove it." Measured against `93b47dfa`, the code has
proved it:

| tier | production | third party |
|---|---:|---|
| `melodia-audio` (`audio.rs`, `decode`, `file_decode`, `stream_decode`, `stream_source`, `prebuffer`, `hls/`, `aac_*`) | 3,510 | symphonia, reqwest, stream-download, icy-metadata. No cpal |
| `melodia-playback` (`output/`, `equalizer`, `replaygain`, `crossfade`, `dsp`, `spectrum`, `visualizer`, `waveform`, `decks`, `stream_health`) | 5,445 with `backend/`, 4,522 without | cpal, biquad, realfft. No reqwest |
| `melodia-engine` (`state`, `queue`, `actions`, `handlers`, `types`, `now_playing`, `event_sink`, `mod.rs`) | 2,779, or 3,702 with `backend/` | neither, once Phase A lands |

Exactly three files import the network (`stream_source.rs`, `hls/reader.rs`, `hls/playlist.rs`) and
exactly three import cpal (`output/device.rs`, `output/mod.rs`, `stream_health.rs`), all three of
them in the lower tier. The dependency sets of the top two tiers do not intersect.

The interface is already written and already argued: `player/audio.rs` is 92 lines with no `crate::`
import at all, and its `//!` says an `AudioSource` is something `output` can pull rather than
something a dependency happens to accept. `output/mod.rs:1` says "everything below the DSP chain".

**The cut costs one file.** `backend/mod.rs:29,31,33` names `file_decode::FileDecoder`,
`prebuffer::StreamShared` and `stream_source::PreparedStream`, which is the correct direction. The
single wrong-direction edge in the whole directory is `file_decode.rs:28`, `use super::dsp::{frames_in,
frames_to_duration, interleaved}`. Those three are pure numeric helpers and belong in `audio.rs`
beside `Sample` and `Shape`, which is where the rest of the shared vocabulary already lives. Move
them and the seam is clean.

What it buys, and this is the one place in the whole issue where the roadmap argument is load
bearing rather than decorative: a podcast episode or a Subsonic track is a new `AudioSource` and
nothing else. Under one `melodia-audio` it can reach `PlayerState`, the mixer and the EQ; under this
split it cannot, and rustc says so at the import. It also makes
`[profile.dev.package.melodia-playback] opt-level = 2` target the DSP chain without also
un-debugging the state machine.

`backend/` is the one judgement call. It holds `PlaybackEngine`, which `PlaybackContext` names, so
engine reads better than playback. If Phase C runs long, the engine cut is the separable one; the
audio/playback cut is not.

### Why `melodia-integrations` exists

Not a preference. `cargo:rustc-env` reaches one crate, and the `option_env!` sites are split across
`scrobble/providers/lastfm.rs` and `discord/mod.rs`, so the two modules share a crate or the build
ships keyless. 3,321 production lines together, which is a reasonable crate on its own terms, and it
keeps `melodia-app` from absorbing every integration the roadmap adds.

### Why `melodia-artwork` depends on `slint`, and `cover_thumbs` moves wholesale

The issue never raises this, and it should not be reopened later without the reasoning.

The line is already drawn inside the file. `cover_thumbs.rs:66` is
`type CachedBuf = Option<SharedPixelBuffer<Rgb8Pixel>>`, and the module doc at `:19` says why:
`slint::Image` is deliberately not `Send`, so the LRU holds buffers and `buf_to_image` builds the
handle at the edge with `Image::from_rgb8(b.clone())`, a refcount bump. What artwork depends on is a
refcounted pixel buffer, the same category of thing as `bytes::Bytes` or `image::RgbImage`. It never
names the renderer, the event loop or a widget.

Two alternatives were checked and both cost more than they save:

- **Store plain bytes so artwork is slint-free.** `Image::from_rgb8` over a `SharedPixelBuffer` is
  O(1); `clone_from_slice` over a `&[u8]` is a memcpy of `thumb_size² × 3` per read. That converts a
  refcount bump into per-row transient allocation, which is the pattern `tasks/material_you.rs:239`
  records as having cost a residual RSS bump the last time it happened. It also breaks the zero-copy
  handoff that comment exists to protect: Material You seeds its palette from the already-decoded
  thumbnail through `get_or_load_rgb8` rather than opening the full-resolution artwork a second time.
- **An extension trait**, buffers and lifecycle in artwork, the four `Image` accessors in views.
  Artwork still needs `slint` for `SharedPixelBuffer`, so it buys naming purity and no dependency
  reduction, and it is an abstraction whose only purpose is to satisfy the split, which is the
  stopping rule the issue sets for itself.

Compile time decides nothing either way: `slint` is on the build critical path through `melodia-ui`
regardless, and cargo compiles it once per workspace. What must not happen is `cover_thumbs` landing
*inside* `melodia-ui`, which exists so the generated unit compiles once; an actively tuned file
(`set_thumb_size`, `row_cover_size`) would rebuild it on every tier tweak. Views is wrong too:
`tasks/material_you.rs:30` threads `Arc<CoverThumbs>` through five signatures, so a views-owned cache
puts app above views and reopens the `state` cycle.

Manifest line is `slint.workspace = true`, never a local version or feature list.

Related placement call, stated rather than left to accident: `logo_tile.rs`'s only consumer anywhere
is `station_logo.rs`, which lands in `melodia-net`. It stays in artwork, since net already depends on
artwork and the alternative puts pixel composition in the crate that owns sockets.

### Why `themes/` splits

The issue puts `themes` in app and an earlier draft of this doc put it in platform; both make a crate
that has no business drawing anything carry `melodia-ui`. The seam is inside the directory. Palette
computation (the registry, the `kdeglobals` and Material You derivations, `on_accent_hex`) names no
Slint type. `apply.rs` is 260 lines and is the only half that does, importing `crate::AppWindow` at
`:8` and writing 19 brushes at `:133`. Palette to platform, `apply` to views. That leaves three
non-views `AppWindow` namers: `tasks/rss_sampler.rs`, which A2 removes; `tasks/updater_daily.rs`,
which lands in app and may keep it; and `services/dwm_titlebar.rs`, which lands in platform and may
not. B8 narrows the third, and only with it does `melodia-platform` depend on core alone.

### What is deliberately not split

- **`src/ui/`.** The issue's argument holds. The twenty slices are a dense mesh, the shared component
  library imports 14 of them, and cutting it needs a view registry, which fails the stopping rule.
- **`library/` as its own crate.** 41 of its 43 non-test files take `&AppState`, so it cannot be a
  leaf. Of 6,086 production lines only 804 are thin `queries::*` wrappers; 1,465 are settings
  persistence that touches no database, 1,521 are radio (which does HTTP), and `playback.rs` and
  `queue.rs` import `player::state` and drive the machine directly. It is a command layer wearing a
  query layer's name, and that renaming is worth an ADR under #84 whatever happens here.
- **Vertical feature crates** (`melodia-podcast`, `melodia-radio`). They are the shape that would
  make a source kind a one-crate change, and they need three inversions the tree does not have:
  `queries::artwork::LEDGER` becoming a registry features contribute to rather than a `const` naming
  their tables, `PlaybackSource` becoming open rather than a closed enum, and the nav index becoming
  data rather than a `NAV_RADIO = 10` const. Revisit if #31 lands commercial, since DRM would force
  an optional feature and there is nothing in the tree to hang one on.
- **Cargo features.** There are zero today, in either manifest, and zero `#[cfg(feature = …)]` sites.
  Keep it that way: `resolver.feature-unification = "workspace"` is nightly-only and the toolchain is
  pinned to stable, so per-crate features reselect and rebuild shared dependencies under any scoped
  invocation, and a real feature matrix is combinations the single `--workspace` gate cannot cover.
  Roadmap features stay runtime settings defaulting off, which is what radio already does.

### What the split does and does not buy

Worth being straight about, because the issue's "Why" section leans on the roadmap harder than the
evidence supports. Radio already reaches eight `src/` directories (`.claude/rules/radio.md` carries
27 path globs across `database`, `entities`, `library`, `media`, `player`, `services`, `tasks` and
`ui`, plus the Slint tree and `migrations/`), and after the split it reaches nearly every crate.
Podcasts will do the same. **A layer split does not make a new source kind cheaper. It makes a new
source kind's misplacement impossible.** Only the second is what the issue actually argues for, and
it stands on its own: tests should verify behaviour and the compiler should enforce topology.

The audio three-way cut is the exception. That one genuinely does make a new source cheaper, which
is the reason it is promoted out of "out of scope".

## Phase A: cut the cycles, still one crate

Independent of each other, so they land in any order, whenever a gap opens. Each is its own commit,
and each ends green on `cargo clippy --all-targets --locked -- -D warnings` then `cargo test --locked`.

- [ ] **A1. `TrackSummary::replaygain()` becomes `From<&TrackSummary>` on the engine side.**
      `entities/track.rs:73-82` moves into `player/replaygain.rs`. Both types are ours, so the orphan
      rule does not bite. One method, one call site, and the `entities` to `player` cycle is gone.
- [ ] **A2. `rss_sampler` takes a closure instead of calling `ui::view_tag`.** `install` at
      `tasks/rss_sampler.rs:59` takes `impl Fn(&AppWindow) -> String`; the binary passes
      `ui::view_tag::format_view` at the `boot` call site. That is the only `crate::ui::` reference in
      all of `src/tasks/`, so it deletes a stated exception rather than relocating it. Delete the
      stale doc comment at `:16-21` in the same commit.
- [ ] **A3. `heap_trim::trim()` moves beside the other platform FFI.** It is a bare
      `libc::malloc_trim` with no task machinery in it, and it is **25 of the 29** `ui` to `tasks`
      edges. `spawn` and `STARTUP_DELAY` stay in `tasks/`, where the one-shot schedule belongs. Not in
      the issue; it is the single cheapest structural win in the list.
- [ ] **A4. The three scan DTOs move to `entities/`.** `ExistingTrackSummary`
      (`database/queries/scan/lookups.rs:70`), `ScannedFile` and `ExtractedMetadata`, applying the
      rule the root `CLAUDE.md` already states. Fixes 7 sites in `database/queries/` plus
      `media/scanner.rs:9` and `:101`.
- [ ] **A4b. The four view-facing DTOs move to `entities/` for the same reason.** `TagEdit`,
      `FieldEdit<T>` and `ArtworkEdit` out of `media/tag_writer.rs`, `StoredLogo` out of
      `media/station_logo.rs`, plus a `library::tags::read_lyrics` wrapper beside `get_tag_edit_rows`
      so `ui/callbacks/tags.rs:127` stops calling the writer directly. This is finding 1, and without
      it the flagship exclusion is false before the first manifest is written.
- [ ] **A5. Persistence and the toast bridge leave `player/`.** Three moves, one commit each:
      - Install `play_count_flusher` in the test contexts that need it, then delete the two
        direct-UPDATE fallbacks at `actions.rs:107-127`, one per counter. `db: &DbPool` then drops
        off `execute_actions` and `emit_and_execute`.
      - Collapse `toast` and `play_count_flusher` onto one `OnceLock<UnboundedSender<E>>` bridge
        primitive. Producer half goes to core, consumer halves stay where the I/O is. Keep
        `try_send`'s `bool`; callers branch on it.
      - The 30 s periodic save (`handlers.rs:443-460`, `PlaybackMonitorContext.db` and `paths`)
        publishes a snapshot the app layer writes, so the monitor stops owning `DbPool` and
        `write_json_atomic_sync`. This is the half that removes `handlers.rs:239` and `:441`.
- [ ] **A6. `describe` and the atomic writers move to core.** `services/mod.rs:351`, plus
      `write_json_atomic_sync`, `write_text_atomic_sync` and `load_json_or_default{,_sync}`. Resolves
      4 of the 12 `player` to `services` edges by relocation. The rest resolve in Phase C by the
      graph, audio depending on net.
- [ ] **A7. `nav_history` and `ui_handles` come off `AppState`.** `state/mod.rs:151,155,277,278,280`
      into a struct the binary owns and passes down. 6 sites in `boot/ui_setup/views.rs`, 18 inside
      `src/ui/` itself. `ui/my_library/tests/my_library_tests.rs:17` does
      `include_str!("../../nav_history.rs")` with source-text assertions at `:1010` and `:1024`, so
      it needs re-pathing. This is the one that actually stops `melodia-views` existing, and the only
      one with structural work in it.
- [ ] **A8. `dsp.rs`'s numeric helpers move into `audio.rs`.** `frames_in`, `frames_to_duration` and
      `interleaved`, whose only cross-tier consumer is `file_decode.rs:28`. That is the one
      wrong-direction edge inside `src/player/`, and removing it is what makes the audio/playback
      seam a manifest line rather than a refactor.
- [ ] **A9. Anchor the test corpus on the workspace root.** `.cargo/config.toml` already has an
      `[env]` table, and `relative = true` resolves against the config file's parent directory on
      stable:

      ```toml
      [env]
      MELODIA_REPO_ROOT = { value = "", relative = true }
      ```

      `""` and not `"."`, measured rather than reasoned. Cargo resolves the value against the parent
      of `.cargo` and never hands the empty string to `Path::join`, so `""` yields `<root>/` and
      joins to `<root>/src`, while `"."` yields a literal `<root>/.` and drags a `CurDir` component
      through every path derived from it and every assertion message that prints one.

      All seven constants in `test_support.rs` take it, as does `minisign.rs:34-35`'s `include_str!`
      and the two directory anchors (`locale_tests.rs:22`, `radio_tests.rs:17`). So does the fourth
      anchor, which is the one A10 stands on: `test_support_tests.rs:51` declares a function-local
      `REPO_ROOT` shadowing the shared constant, so re-pointing the seven leaves the rules-glob pin
      looking at whichever crate it lands in. Landing it before anything
      moves is what stops `SRC_DIR` ever meaning "one crate".
- [ ] **A10. Make the rules-glob pin honest.** Drop `if glob.contains('*') { continue; }` at
      `test_support_tests.rs:89` and fix whatever it was hiding. On its own commit, before the split,
      so a glob that had already rotted cannot be mistaken for one the split broke. The floor's
      comment at `:52` is stale while you are in there: it says sixty literal paths, and the ruleset
      holds 96.
- [ ] **A11. `[lib] test = false` on `melodia-ui`.** `melodia-ui/src/lib.rs` is
      `slint::include_modules!()` and a re-export with no `#[test]` in it, but `--workspace` selects
      it and both `cargo test` and `--all-targets` then build its lib as a unit test, which is a
      second compilation of the 411,428-line generated file the crate exists to build once. Clippy
      still lints the lib through `RUSTC_WORKSPACE_WRAPPER`, so this loses no coverage, and it is
      what makes carrying `--workspace` from the first commit actually free.

## Phase B: reshape in place, still one crate

- [ ] **B1. Split monolithic `services/mod.rs`** (376 lines). It is simultaneously the
      core-primitives module (`load_json_or_default*`, `write_*_atomic_sync`, `current_exe`,
      `is_dev_build`, `redact_home`, `home_dir_string`, `describe`) and the HTTP module
      (`build_http_client`, `http_url`, `is_http_url`, `is_http`, `get_capped`, `get_capped_text`,
      `read_capped`), nine and seven exports respectively. **Hard prerequisite for everything else**:
      all four media fetchers plus `radio_browser`, both scrobble providers, `updater/github` and
      `player/hls/` depend on the net half, so nothing can move before this does.
- [ ] **B2. `services/` regroups into net, platform, integrations and app.** `updater/`'s 24 files
      straddle all of them and do not move wholesale: net is `check`, `github`, `manifest`,
      `install/download`; platform is `target`, `linux_pkg`, `system_install`,
      `install/{staging,verify,swap}`; core is `minisign`, `version`, `probe`; the rest is
      orchestration. `scrobble/` and `discord/` go to integrations together (finding 5).
      `artist_images.rs` goes to app, not net (finding, `What validation changed`). `diagnostics.rs`
      names `database` and `state`, so it is app too.
- [ ] **B3. `media/` regroups three ways.** Image tier, ingest, fetchers. Assign `rating_tags.rs` to
      ingest, and pull `services/material_you.rs` into the image tier.
- [ ] **B4. `single_instance.rs:31`'s `crate::media::is_audio_extension`** is the one import between
      that file and a dependency-free platform module. Take the predicate to core.
- [ ] **B5. Move the cross-tier size assertions out of the image tier.**
      `media/artwork/tests/artwork_tests.rs:119-121` reach `ui::grid_prewarm` and `ui::util` to check
      `STORE_MAX_DIM` against the UI's cover tiers. They cannot compile inside a leaf crate, so they
      become integration tests under `crates/melodia/tests/`.
- [ ] **B6. `themes/` splits palette from apply**, per the graph section. `apply.rs` is the views
      half; everything else is the platform half.
- [ ] **B7. `player/` regroups into `source`, `output` and `engine` modules** ahead of the manifests,
      so the three-way extraction in Phase C is a move rather than a design decision made under a
      compile error. `output/` already exists and needs no change.
- [ ] **B8. `dwm_titlebar` splits, and only its lower half is platform.** `apply`
      (`services/dwm_titlebar.rs:41`) needs a window handle and a `u32` colour, and `win32_hwnd` at
      `:76` is the only reason *it* names `crate::AppWindow`: a `WinitWindowAccessor` hop the caller
      can do. `reapply_from_theme` at `:50` does not follow it down, because it reads `Theme.mantle`
      back off the Slint global and so names `crate::Theme` as well; that half belongs in views
      beside `themes::apply`. Both callers already sit above platform (`main.rs:466`, and
      `themes/apply.rs:171`, which has `p.mantle` in hand and needs no read-back), so nothing has to
      be threaded. Moving the hop is what keeps `melodia-platform` off `melodia-ui`, and it is
      invisible on the two platforms it does not compile on, so it lands before the manifests rather
      than under one.

## Phase C: extract the crates

Order, each landing green: **core, testkit, artwork, net, platform, audio, playback, engine, store,
integrations, app, views, bin.**

- [ ] Move every dependency more than one member names into `[workspace.dependencies]` first
      (finding 12), starting with `reqwest`, `stream-download`, `icy-metadata`, `sqlx`, `symphonia`,
      `cpal`, `tokio`, `serde`, `image`, `log`, `blake3`. Members take `<dep>.workspace = true`.
- [ ] Per-member manifest, modelled on `melodia-ui/Cargo.toml`, which already gets this right:

      ```toml
      [package]
      name = "melodia-<x>"
      version.workspace = true       # never "0.0.0"; see finding 3
      edition.workspace = true
      rust-version.workspace = true
      license.workspace = true
      repository.workspace = true    # new entry in [workspace.package]
      publish = false

      [lib]
      doctest = false                # matters more once --workspace is the default

      [lints]
      workspace = true               # omitting this silently leaves the gate
      ```

- [ ] The per-crate build-script work, because these are compile errors the moment a boundary
      appears: `sqlx::migrate!`'s path, `minisign.rs`'s `include_str!`, `radio_blocklist`'s `OUT_DIR`
      bake and its two dotfiles, `load_dotenv()` moving to `melodia-integrations`, and the Windows
      `winresource` embed staying with the binary with its `assets/melodia.ico` hop rewritten.
- [ ] `test_support` becomes `melodia-testkit` **with no workspace dependency** (finding 2).
      `paths_in()` goes to `melodia-core`'s own `#[cfg(test)]` support and `resolved_home()` to the
      crate owning `home_dir_string`.

**Landing tactic for the 2,721 `crate::` paths.** Each consuming crate re-exports what it took
(`pub use melodia_core::{error, config, entities, utils};` in its `lib.rs`), so `crate::error::AppError`
keeps resolving and the diff stays about topology rather than import churn. Enforcement is unaffected
for the crates a member does not name at all: a re-export cannot reach a crate absent from the
manifest. It *is* affected the other way, so finding 13's rule applies from the first re-export:
never `pub use` a type out of a crate your dependents are meant to be unable to reach. De-facade in
Phase D once the graph is proven, one crate at a time.

## Phase D: make the repo workspace-native

- [ ] Virtual root manifest, `members = ["crates/*"]`, `exclude = ["winit"]`. No `default-members`
      (finding 11). Profiles and `[patch.crates-io]` stay at the root, as do the four version scrapes.
      `melodia-ui` moves to `crates/melodia-ui/` unchanged; that is what keeps the 135 Slint
      `include_str!` hops resolving from files at the same depth.
- [ ] `[[bin]] name = "Melodia"` on the binary crate (finding 4), so the artifact name survives.
- [ ] `[package.metadata.deb]` moves to the binary crate with all eight asset paths and `license-file`
      rewritten, then a real `cargo deb --target … --no-build` to confirm the `target/release/` prefix
      rewrite still fires. Re-key `LICENSE_SHIPPERS`' `("Cargo.toml", ...)` entry in
      `services/tests/mod_tests.rs:398`, which fails loudly and correctly when it moves.
- [ ] `release-build.yml:83` and `:124` take `-p melodia`; `:143`'s `cargo wix --package` follows.
      A bare `cargo build` at a virtual root builds every member.
- [ ] `tests/` moves to `crates/melodia/tests/`, and every corpus walk moves with it (findings 6 and
      8): the `rfd` pin, `current_exe`, thread-name length, the single-resampler equality,
      `CALLBACK_HOMES`, the scrollbar brace-matching, the rules-glob pin,
      `no_result_carries_its_error_as_a_string`, and both halves of the radio off-switch pin. One
      home, one reach, walking `$MELODIA_REPO_ROOT/crates`.
- [ ] The 243 relative `include_str!` hops, as they surface.
- [ ] All 134 `.claude/rules` globs (83 naming `src/`, 43 `melodia-ui/`, 5 `tests/`, 3 `build.rs`),
      against the pin A10 already made honest.
- [ ] `CLAUDE.md`'s module map and its "every path below is `src/`-relative" convention, the README
      architecture section, `src/player/CLAUDE.md`'s heading, and the 162 bracketed intra-doc links
      (`\[\`?crate::`).
- [ ] Drop the scope-clippy-to-one-crate convention. `feature-unification = "workspace"` is
      nightly-only and the toolchain is pinned to stable, so a scoped invocation reselects features
      for shared dependencies and rebuilds them. `--workspace` on all three gate commands is the only
      correct form, which a workspace makes the natural one anyway.
- [ ] Measure the CI test job. It caps `CARGO_BUILD_JOBS: 4` with a comment naming five test binaries
      and warning that past the ceiling the runner swaps rather than fails; the split takes that to
      roughly fourteen.

## Verification

Per commit. Only `cargo fmt --all` is workspace-wide today; the other two select the root package
and grow a `--workspace` in Phase D. Carrying it from the first commit is free once A11 lands, and
not before:

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked --workspace -- -D warnings
cargo test --locked --workspace
```

Phase boundaries:

- After **A**: `grep -rn 'crate::database\|crate::tasks' src/player/` returns nothing, and
  `grep -rn 'crate::services' src/player/` returns only the HTTP primitives. `grep -rn 'crate::ui' src/tasks/`
  returns nothing. `grep -rn 'crate::media' src/ui/` returns only the image tier.
- After **C**: delete a `path` dependency from one manifest and confirm rustc names the crate in the
  error. **The `melodia-views` manifest must list neither `melodia-store` nor `melodia-net`**, the
  `melodia-audio` manifest must not name `cpal`, and `melodia-platform` must not name `melodia-ui`.
  Those are the flagship rules turning into compile errors, and they are the whole point of the
  exercise.
- After **D**: `cargo deb -p melodia` plus `scripts/build-{rpm,appimage,tarball}.sh` each produce an
  artifact and the produced binary is still named `Melodia`; `cargo build --timings` against the
  prerequisite baseline; `/usr/bin/time -v target/release/Melodia` for peak RSS. No RSS change is
  expected, `lto = "fat"` with `codegen-units = 1` recovering cross-crate inlining.

One thing gets better rather than staying level: `[profile.dev.package.melodia-playback] opt-level = 2`
becomes possible, so the DSP chain stops being debugged unoptimized, without also optimizing the state
machine out from under a breakpoint.

## Notes

**239 `pub(crate)` sites**, 228 of them outside a `tests/` directory and 193 outside
`test_support.rs` as well, widen to `pub` where they cross a boundary, against 2,254 `pub` items
(`^\s*pub (async |unsafe )*(fn|struct|enum|const|type|mod|trait|static)`, tests excluded; dropping
the `async` arm loses exactly 306 of them). 193 is the number that costs anything: the other 35 are
`test_support` itself, which goes `pub` in `melodia-testkit` regardless. The issue counted 209; the
tree has grown since. That is the price the literature names, and it is the same thing as the
payoff: it forces the interface to be stated.

**Prior art is argued in the issue** and not restated here. The four sibling checkouts kept beside
this repo (rox, termusic, Symphonia, sonora) are what its crate counts can be checked against.
Thirteen members is above the issue's nine and still below librespot's eight-plus-protocol at a
comparable size, because the audio stack is three rather than one and integrations is its own.

**`cargo-crate-split` is on crates.io at 0.2.0.** It computes strongly connected components and emits
a minimum cut set, suggest-only, never rewriting source, which suits the no-autofix rule. Its blind
spot is glob re-exports and inference-hidden coupling, so treat its list as a floor cross-checked
against the cycles above rather than as the answer.

**What this trades away, plainly:** a single `src/` tree any grep reaches in one pass, a test corpus
that has been genuinely good at catching drift, and a packaging path that works today. The bet is
that a compiler-enforced DAG is worth more over the next two years of podcasts and streaming than
those three are, and that the cost is paid once.
