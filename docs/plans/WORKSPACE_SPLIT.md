# Splitting `src/` into a workspace

Working doc. Validation of the issue, the dependency graph as the code actually has it, and the
order the cuts come out in. Harvest into `docs/adr/` when
[#84](https://github.com/KenanSalar/Melodia/issues/84) ships, not before: the boundary rationale
below is exactly what #84 exists to stop evaporating.

Status: **Phases A, B, C and D complete; Phase E open** · Issue:
[#83](https://github.com/KenanSalar/Melodia/issues/83) · Created: 2026-09-03 · Validated against
`93b47dfa`, Phase A landed on `a1c087e4`, Phase B over five commits from `e506b490`, Phase C over
four from `1060d8f1` to `01c314a8`, Phase D over seven from `37cad90f` to `8cbbe055`

> **Phase A's twelve items are done and its checks pass**, so the counts below that describe
> `src/` describe the tree *before* it. Where a Phase B item's inventory has moved, its own entry
> says so and carries the remeasured one; nothing else here has been re-derived.

> **Phase B was re-read against the tree before its first commit**, and the pass moved four crate
> boundaries, corrected ten claims and found four edges no item carried. The Phase B items below
> are the rewritten ones, renumbered into the order they land in; findings 18 to 21 carry what had
> no item at all. The After-A checks were re-run on that same read and all six still pass.

> The issue body carries the argument for *why* a workspace. This doc carries what a read of the
> tree found that the body does not, and it is the source [#84](https://github.com/KenanSalar/Melodia/issues/84)
> will draw on, so the rationale for each boundary belongs here rather than in the issue thread.
> Where this doc and the code disagree, the code is right.

## Prerequisites

- [x] Radio ships.
- [x] #79 ships, `PlaybackSource` included. Closed 2026-09-01; `src/player/CLAUDE.md` documents
      `source_allows(PlaybackSource::advances_queue)` and the five surviving `is_radio` sites.
- [x] `cargo build --timings` on a clean target, so the extraction phases can be judged rather than
      assumed. Taken at C1, the last point at which the "before" is one crate: **4m 03s wall over
      766 units and 1,330 unit-seconds**, the four heaviest being `melodia-ui` at 79s,
      `aws-lc-sys`'s build script at 57s, `i-slint-compiler` at 49s and **`Melodia` itself at 33s**.
      That last one is the number Phase D compares against, being the single unit the split turns
      into twelve that can compile at once; the three above it are dependencies the split does not
      move and cannot improve.

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
| ~134 of 225 `.claude/rules` globs break | **83 of 165**, and 134 of 165 once `melodia-ui`, `tests/` and `build.rs` move too. Re-measured at C1 the ruleset had grown to 169: 87 `src/`, 43 `melodia-ui/`, 5 `tests/`, 3 `build.rs`, so **138 of 169** |
| 21 test files anchor on a tree root | **28**: 27 through the seven constants, plus `library/tests/radio_tests.rs:17` direct |
| 241 relative `include_str!` across 46 files | **243 across 47 files** (254 literal-arg sites across 48, but the 11 without a `../` are `minisign_tests.rs`'s crate-local `fixtures/`); the 135 reaching `melodia-ui/ui/` is exact |
| ten non-binary `CARGO_PKG_VERSION` sites | 10 expansions, **9 non-binary, 7 non-test**, and `ui/callbacks/updater/install.rs:83` is missing from the list |
| `src/` is 81,131 production lines | **86,820**, of 133,772 total. The table below is remeasured |

Four edges the issue's table omits, each a real call and not a doc link:

- `media/metadata.rs:212` calls `player::file_decode::probe_duration`, so **store depends on audio**.
  It is the sole edge `media/` has into that directory, which `src/player/CLAUDE.md` already says,
  and the sole edge `media/` has into anything but `entities`, `error`, `services` and itself.
- `station_logo.rs:140` and `deezer.rs:245` call `artwork::store_image`, so **net depends on artwork**.
- `scanner.rs:52` and `metadata.rs:178` take `&artwork::CoverCache`, so **store depends on artwork**.
- `services/artist_images.rs:9-12` names `database::{DbPool, queries}` *and* `media::deezer`, so a
  "net owns everything that opens a socket" rule would put **store on net's dependency line**. It is
  an orchestrator rather than a fetcher and belongs in app; see finding 3.

Four files outside `src/ui/` name `crate::AppWindow`, not the two the issue lists: `themes/apply.rs:8`,
`tasks/updater_daily.rs:47`, `tasks/rss_sampler.rs:47` and `services/dwm_titlebar.rs:24`
(Windows-gated). Two of the four survive the plan below: `updater_daily`, which lands in app and may
name it, and `dwm_titlebar`, which lands in platform and may not, so B4 narrows it.

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

   **Closed by A4b.** `grep -rn 'crate::media' src/ui/` now returns only `cover_thumbs`, `artwork`
   and `image_decode`, so the exclusion is true for the first time and stays checkable by grep
   until Phase C makes it a manifest.
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
14. ~~**`toast` and `play_count_flusher` are the same primitive written twice.**~~ **Done in A5b**,
    as `utils::event_bridge`. Both producers moved to core with it, which the finding did not
    anticipate and the After-A check forced: extracting the primitive alone left `player/` naming
    `tasks` and `services` to reach the senders. The asymmetry the finding says not to flatten
    survived — `try_send` returns `bool`, `notify` `()`.
15. ~~**`player` names `DbPool` for a test fallback in one file and for real in another.**~~
    **Done in A5.** The reading was right about `handlers.rs` and wrong about `actions.rs`: the
    fallback's comment claims a test contract no test has, so it was a deletion with no fixture
    work in front of it rather than an install-then-delete. `src/player/` now names `DbPool`
    nowhere, and lost its `config::Paths` edge with it.
16. ~~**Stale doc comment** at `tasks/rss_sampler.rs:16-21`.~~ **Done in A2**, which deleted the
    paragraph rather than correcting it: the exception it documented is gone, `src/tasks/` naming
    nothing under `ui::` and no `AppWindow` outside `updater_daily`.
17. **Nothing in the gate runs `cargo doc`.** The 162 bracketed intra-doc links that will stop
    resolving degrade silently to plain text rather than failing. Either accept that explicitly or
    add a `cargo doc` step, but do not leave it unstated.
18. **`media_controls` cannot be platform, and the reason it is not is that it is the shape of
    Discord.** `media_controls/mod.rs:11-14` names four `crate::player::` types
    (`EventSink`/`MediaControlsSync`/`PlayerEvent`, `SourceSummary`, `PlayerViewModelLight`,
    `PlaybackStatus`) and `:334` calls `player::state::volume_to_amplitude`, so a
    `melodia-platform` that owns it depends on engine, and with it on playback and cpal, under
    every crate that leans on platform for a tray icon. The placement that costs nothing is
    already in the graph: publish now-playing state to a surface outside the app, take transport
    commands back, is exactly what `discord/` does, and `integrations -> engine` is an edge that
    exists. **`media_controls` is an integration.** `always_on_top/` reads as the same problem and
    is not: its `&AppState` (`mod.rs:129`, `kwin.rs:21`) is two field reads,
    `state.paths.data_dir` and `state.always_on_top.method`, so narrowing the two parameters
    leaves a clean platform module rather than moving it.
19. **The updater is a feature, and splitting it four ways to satisfy a layer diagram is the
    abstraction this doc's own stopping rule forbids.** `melodia-app` depends on net, platform and
    core, so an updater living whole in app violates nothing, and `install/` is a four-file
    sequence (download, staging, verify, swap) whose cohesion is real. What does have to leave is
    what has a *second consumer*, which is the only evidence that separates a platform primitive
    from a feature's internals: `crash_report.rs:198` and `desktop_integration.rs:32` both reach
    `updater::{target::current_target_key, install_target, linux_pkg}`. That set closes under
    `probe` and `system_install` and under nothing else, so **`{install_target, target, linux_pkg,
    probe, system_install}` goes to platform as `install_kind` and the rest of `updater/` stays
    together in app**. Three of the first pass's per-file calls were wrong besides: `manifest.rs`
    is core rather than net (`serde` and `HashMap`, no `reqwest`), `probe.rs` is platform rather
    than core (`create_new` plus `process::id()`), and `asset_cache.rs` was unlisted and is core.
    The directory holds 16 source files, not 24.
20. **`melodia-views` names `melodia-integrations`, and the graph should say so.**
    `ui/settings/discord_settings.rs:18` takes `DiscordStatus`, and
    `ui/settings/scrobbling_settings.rs:26-27` takes `providers::{lastfm, listenbrainz}` plus four
    more types, then drives the whole Last.fm and ListenBrainz connect flow inline
    (`get_token`, `get_session`, `validate_token`). Routing that through an app-layer facade would
    buy one manifest line and cost a wrapper whose only purpose is the diagram. Views already
    depends on app, app already depends on integrations, so the edge closes no cycle and adds no
    reach: **the graph gains `views -> integrations`.**
21. **Three leaf predicates sit one layer above where anything needs them.** ~~`is_dev_build` is
    called from `config.rs:73`, so **`melodia-core` names `melodia-services` today**, which is a
    cycle no item in the first pass records.~~ **The cycle went in B1**, which moved `is_dev_build`
    to `utils/exe.rs`; `config.rs:73` still calls it and the call is now core-internal. `media::is_audio_extension` is a dependency-free
    `eq_ignore_ascii_case` fold that `services/single_instance.rs:31` reaches for, and it is that
    file's only `crate::` path of any kind. `media/self_writes.rs` has no `crate::` import at all
    and only `parking_lot`, and its consumers are `library/{mbid,tags}.rs` and
    `tasks/file_event_processor/`, so it is grouped with the lofty-heavy ingest modules by
    adjacency rather than by membership. All three belong in core, and moving them also clears
    three test-only edges the tiers cannot carry (`player/tests/file_decode_tests.rs:61` and
    `services/tests/mod_tests.rs:613,626` both read `media::AUDIO_EXTENSIONS`).

**Survives untouched**, verified, so no work is planned for it: the four `[workspace.package]`
version scrapes (they anchor on `^\[`, so the `[package]` table vanishing is fine),
`pr-validation.yml`'s `changes` filter (a denylist rooted at `'**'`, so `crates/**` falls through
correctly), every `$REPO_ROOT`-based path in the packaging scripts, and `wix/main.wxs`'s
`$(sys.SOURCEFILEDIR)..`.

## The graph, corrected

Thirteen members plus `melodia-ui`. Four changes from the issue's nine, each resting on a seam the
code already has rather than on an abstraction invented to satisfy the split.

```
melodia-core          error, config, entities (+ the boundary DTOs and the
                      two settings flag structs), utils, themes whole,
                      describe, atomic json/text writers, the binary-path
                      and home-redaction pair, is_audio_extension,
                      self_writes                                          -> nothing
melodia-testkit       env lock, path anchors, corpus walkers               -> nothing (dev-dep of all)
melodia-artwork       artwork/, cover_thumbs, image_decode, logo_tile,
                      material_you                                         -> core, slint
melodia-net           http primitives, 4 fetchers, radio_browser,
                      radio_blocklist + its bake                           -> core, artwork
melodia-platform      tray, single_instance, allocator, logging,
                      crash_report, system_theme, desktop_integration,
                      always_on_top, dwm_titlebar's lower half,
                      the updater's install-kind sliver                    -> core
melodia-audio         audio.rs vocabulary, decode, file/stream/hls
                      sources, aac                                         -> core, net
melodia-playback      output device/mixer/convert, EQ, ReplayGain,
                      crossfade, dsp, spectrum, visualizer, waveform,
                      decks, stream_health                                 -> core, audio
melodia-engine        PlayerState, queue, actions, handlers, types,
                      now_playing, event_sink, backend                     -> core, audio, playback
melodia-store         database/, scanner, metadata, watcher, tag_writer,
                      rating_tags                                          -> core, artwork, audio
melodia-integrations  scrobble, discord, media_controls, and load_dotenv
                      with them                                            -> core, net, engine
melodia-app           library/, tasks/, state/, settings, view_state,
                      artist_images, diagnostics, the updater whole but
                      for its install-kind sliver                          -> all above, melodia-ui
melodia-views         src/ui/ (incl. the brush half of themes::apply)      -> app, engine, playback,
                                                                              artwork, platform,
                                                                              integrations, core,
                                                                              melodia-ui
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

Four further deltas, each from a code edge the first pass did not walk, argued as findings 18 to 21:
`media_controls` is an integration rather than a platform service, the updater stays whole in app
but for one sliver, `melodia-views` names `melodia-integrations`, and core grows the three leaf
predicates that were sitting one layer too high.

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
`:8` and `crate::Theme` at `:9`, and writing 19 brushes at `:133`. Palette to platform, the brush
half to views. **The seam is inside `apply.rs` too, not only inside the directory**: about 55 of its
lines are palette computation with no Slint type in any signature, `on_accent_hex` among them, which
is why B3 is a three-way split and `themes/tests/apply_tests.rs` (already gated to the
`palette_from_kde` half) travels with them. That leaves three non-views `AppWindow` namers:
`tasks/rss_sampler.rs`, which A2 removes; `tasks/updater_daily.rs`, which lands in app and may keep
it; and `services/dwm_titlebar.rs`, which lands in platform and may not. B4 narrows the third, and
only with it does `melodia-platform` depend on core alone.

**Where the palette half lands is core, not platform, and C2 corrected that.** Two things were wrong
with platform. It is a cycle: `themes/system_color_state.rs:23` and `themes/kde.rs:139` both take
`platform::system_theme::KdeColorPalette` while `system_theme.rs:112,121` take
`themes::SystemColorState`, which cargo cannot express across two crates. And platform is the crate
that owns the tray, zbus, ksni and libc, so putting the registry there makes `melodia-artwork` — a
pure image crate — depend on the OS-integration crate to name sixteen `u32`s
(`media/image/material_you.rs:33`). What is left of `themes/` after B3 is static tables, four
plain-data structs and pure functions over them, which is `entities/`'s category rather than a
service's, and in core it sits below all four of its consumers. The cycle breaks by moving
`KdeColorPalette` into `themes/kde.rs` beside the `palette_from_kde` that consumes it; platform
keeps the half that is a platform concern, which is reading `kdeglobals` and the portal and
*producing* those two types.

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

- [x] **A1. `TrackSummary::replaygain()` becomes `From<&TrackSummary>` on the engine side.**
      `entities/track.rs:73-82` moves into `player/replaygain.rs`. Both types are ours, so the orphan
      rule does not bite. **Three call sites, not one** — `handlers.rs:167`, `state.rs:513` and
      `:853` — but all three already sit inside `src/player/`, so the cycle goes with no
      cross-directory churn. Each spells `track.as_ref().into()`, the receiver being an `Arc`.
- [x] **A2. `rss_sampler` takes a closure instead of calling `ui::view_tag`.** Takes
      `impl Fn() -> Option<String>` rather than `impl Fn(&AppWindow) -> String`: the `&AppWindow`
      form leaves `crate::AppWindow` in the signature, and the "Why `themes/` splits" section counts
      on this item removing the sampler from the non-views `AppWindow` namers. The closure captures
      the weak handle and answers `None` once the window is gone, which is what ends the loop.
      The call site is **`main.rs:360`**, not `boot/` — `boot/tasks.rs` never touches the sampler.
      Its doc comment was stale on its own terms besides, naming two imports (`Nav`,
      `ui::window_chrome::is_queue_sheet_open`) that had already moved into `ui::view_tag`.
- [x] **A3. `heap_trim::trim()` moves beside the other platform FFI.** It is a bare
      `libc::malloc_trim` with no task machinery in it, and it is **25 of the 29** `ui` to `tasks`
      edges, leaving 4 (three `TaskSpawner` imports and one `radio_logo_cache::spawn`). `spawn` and
      `STARTUP_DELAY` stay in `tasks/`, where the one-shot schedule belongs.

      **There was no "other platform FFI" to move it beside.** `libc::` appears in exactly two
      places in the tree, this call and `main.rs`'s three `mallopt`s, and `Cargo.toml`'s dependency
      comment documents `libc` as existing for exactly those two. So the landing spot is new —
      `services/allocator.rs`, holding both, since they are one concern (pinning the trim threshold
      is what leaves the trim anything to do) and `libc` becomes a single module's dependency for
      `melodia-platform`'s manifest line. `main()` calls `pin_arenas_and_thresholds()` where the
      block was; a call allocates nothing, so the ahead-of-the-first-malloc invariant holds.
      `CLAUDE.md`'s "literal first statement in `main()`" was already wrong — `--version`,
      `--logs` and the updater reap all precede it — and now says what the code says.
- [x] **A4. The three scan DTOs move to `entities/`.** `ExistingTrackSummary`
      (`database/queries/scan/lookups.rs:70`), `ScannedFile` and `ExtractedMetadata`, into one new
      `entities/scan.rs` — they are the scan pipeline's vocabulary and `ScannedFile` holds an
      `ExtractedMetadata`, so splitting them across two modules buys nothing. All three are
      `#[derive]`-only plain data with no impls and no lofty or sqlx type in any field; the reverse
      edge in `media/tests/scanner_tests.rs` goes with them, which the item's site list missed.
- [x] **A4b. The four view-facing DTOs move to `entities/` for the same reason.** `TagEdit`,
      `FieldEdit<T>` and `ArtworkEdit` into `entities/tags.rs` with `TagEdit`'s four predicates;
      `StoredLogo` into the existing `entities/radio.rs`, being a station's. Plus a
      `library::tags::read_lyrics` wrapper beside `get_tag_edit_rows`, kept sync — the caller owns
      the `spawn_blocking`, having a runtime handle where the wrapper does not.

      **The flagship exclusion now holds**: `grep -rn 'crate::media' src/ui/` returns only
      `cover_thumbs`, `artwork` and `image_decode`. `UnsupportedFields` is a fifth companion type
      the item does not assign; it is named nowhere outside `tag_writer.rs` itself, so B5 leaves it
      there.
- [x] **A5. Persistence and the toast bridge leave `player/`.** Three moves:
      - ~~Install `play_count_flusher` in the test contexts that need it, then~~ delete the two
        direct-UPDATE fallbacks. **There was no such test context** — nothing anywhere runs
        `execute_actions` on `UpdatePlayCount`/`UpdateSkipCount`, and the DB behaviour is tested
        directly against `queries::track`. So it is a straight deletion, and `actions_tests.rs`'s
        fixture loses the `DbPool` that existed only to satisfy the parameter, which makes its
        builder sync. `db: &DbPool` drops off `execute_actions` and `emit_and_execute`.
      - Collapse `toast` and `play_count_flusher` onto `utils::event_bridge::EventBridge<E>`.
        The two installers stay different and that is the substance: the flusher's `spawn` claims
        the bridge *and* spawns its consumer, where `toast::init` hands its receiver to a UI-thread
        task. `try_send` keeps its `bool`, `notify` its `()`.

        **Extracting the primitive is only half of "producer half goes to core", and the After-A
        check is what says so.** Left where they were, `player/actions.rs` still named
        `crate::tasks::play_count_flusher` and `crate::services::toast` — the two edges the check
        forbids. So both producers move: `services/toast.rs` is producer end to end (its consumer
        is `boot::ui_setup`'s) and becomes `utils/toast.rs` whole, while `play_count_flusher`
        splits, the enum and the send going to `utils/play_counts.rs` and the batching staying in
        `tasks/`. The rules-glob pin A10 had just made honest caught the stale
        `src/services/toast.rs` entry in `ui-patterns.md` on the first run after the move.
      - The 30 s periodic save publishes a `PlaybackSnapshot` through a `SnapshotSink`, which
        `tasks::playback_monitor` supplies — awaited inline, so an in-flight save still completes
        before shutdown wins the next select. Takes `src/player/` to **zero** `DbPool` references
        and removes the whole `player` to `config::Paths` edge, which the item does not claim.
- [x] **A6. `describe` and the atomic writers move to core.** `describe` lands in `src/error.rs`,
      beside the type it reads, widening from `pub(crate)` to `pub`; its test moves out of
      `services/tests/mod_tests.rs` with it. The four file primitives land in a new
      `src/utils/atomic_file.rs` (`utils.rs` becoming `utils/mod.rs`), where the module name carries
      the argument the reads rest on: they are plain and unsynchronised, and safe because every
      write goes through a temp file and a rename. The writers drop the now-stuttering `_atomic`
      infix — `atomic_file::write_json_sync`, `write_text_sync`.

      Resolves 4 of the 12 `player` to `services` edges by relocation, leaving 5 HTTP primitives, 2
      toast (A5's) and one intra-doc link. **The count is order-dependent**: `handlers.rs:452` is
      one of the four and is inside the block A5 deletes, so landing A5 first makes this 3.

      78 call sites, the churn-heaviest item in Phase A: 59 for `describe` and 19 for the file
      primitives. Four `services/mod.rs` items are equally core-shaped and deliberately stay put
      (`current_exe`, `is_dev_build`, `redact_home`, `home_dir_string`) — B1 owns the full split and
      moving them here would half-do it.
- [x] **A7. `nav_history` and `ui_handles` come off `AppState`.** Both into one `Navigation` in
      `ui/nav_history.rs`, holding the stack and a `SectionHandles` — renamed from `UiHandles`,
      which collided with `boot::ui_setup::views`' unrelated struct of that name. `src/state/`
      now names nothing under `ui::`, which is what stops `melodia-views` existing.

      **Reached through a `LazyLock` static, not a value the binary threads down.** Threading was
      the plan and it does not survive contact: `Navigation` is read from twelve `'static`
      callbacks and five `open_*_with` futures, and carrying it there means a parameter on roughly
      fifty functions — `open_album`, `open_album_with`, `seed_detail_from_settings`, two `wire`
      hops and every grid, lifecycle and cross-tab caller, five times over — to arrive at a value
      there is exactly one of per process. A static instead *removes* a parameter from fourteen
      functions rather than adding one to fifty, and it is the bargain `utils::toast`'s bridge
      and `window_chrome`'s geometry statics already take. It holds no `AppState`, so nothing in
      it can grow a second way back to the app layer, which is the property the split needs.

      Two source-text pins moved with it, both in `my_library_tests.rs`. The `record_current`
      spelling lost its state argument. And the `land_pending` pin told its two roles apart by
      which handle each read — the bail taking the caller's `state`, the failure arm its clone —
      a distinction that is gone now neither reads one; it reads the role off what *follows* the
      call instead, only the bail returning. Both mutation-tested.
- [x] **A8. `dsp.rs`'s numeric helpers move into `audio.rs`.** `frames_in`, `frames_to_duration` and
      `interleaved`, whose only cross-tier consumer is `file_decode.rs:28`. That is the one
      wrong-direction edge inside `src/player/`, and removing it is what makes the audio/playback
      seam a manifest line rather than a refactor. `output/voice.rs`'s three calls are same-tier and
      were never edges; they lose their `dsp::` qualifier and nothing else.
- [x] **A9. Anchor the test corpus on the workspace root.** `.cargo/config.toml` already has an
      `[env]` table, and `relative = true` resolves against the config file's parent directory on
      stable:

      ```toml
      [env]
      MELODIA_REPO_ROOT = { value = "", relative = true }
      ```

      `""` and not `"."`, measured rather than reasoned. Cargo joins the value onto the parent of
      `.cargo`, so `""` yields `<root>/` **with a trailing separator** while `"."` yields a literal
      `<root>/.` and drags a `CurDir` component through every derived path and every assertion
      message that prints one. Measured again on the way in, because the shape decides the call
      sites: the suffixes spell no leading separator of their own (`concat!(env!(…), "src")`), one
      being what turns `<root>/src` into `<root>//src`.

      All seven constants in `test_support.rs` take it, as does `minisign.rs:34-35`'s `include_str!`
      and the two directory anchors (`locale_tests.rs:22`, `radio_tests.rs:17`). So does the fourth
      anchor, which is the one A10 stands on: `test_support_tests.rs:51` declares a function-local
      `REPO_ROOT` shadowing the shared constant, so re-pointing the seven leaves the rules-glob pin
      looking at whichever crate it lands in. Landing it before anything
      moves is what stops `SRC_DIR` ever meaning "one crate".

      Twelve sites, not eleven: `tests/headless.rs:33` is the one this list missed, and it is the
      one the approach does not cover for free — an integration test outside `src/` cannot reach a
      `pub(crate)` constant, so it spells `env!("MELODIA_REPO_ROOT")` itself.
- [x] **A10. Make the rules-glob pin honest.** ~~Drop `if glob.contains('*') { continue; }` at
      `test_support_tests.rs:89` and fix whatever it was hiding.~~ **Both halves of that were
      wrong.** Nothing was hidden: all 69 globs matched and all 96 literals existed. And dropping
      the skip alone breaks the test, because `:93` is `Path::join(glob).exists()` — a literal path
      check with no glob semantics, which all 69 would have failed. The item is *implementing a
      matcher*: one question asked of all 165 entries, over a `glob = "0.3.4"` dev-dependency,
      rather than hand-rolling `**` inside the one test that exists to catch silent drift.

      What that surfaced, and the reason it was worth its own commit: **`glob` reads a trailing
      `/**` as subdirectories alone**, so `packaging/**` and `licenses/**` — neither of which has a
      subdirectory — read as rules that had rotted. The rule loader means "everything under here",
      so the pin normalises that one shape to `/**/*`. Three entries use it; every other glob in
      the ruleset is `dir/**/*.ext` or `dir/*.ext` and needs nothing.

      The floor's comment at `:52` was stale as noted (sixty against 96) and is now stated as 165.
- [x] **A11. `[lib] test = false` on `melodia-ui`.** `melodia-ui/src/lib.rs` is
      `slint::include_modules!()` and a re-export with no `#[test]` in it, but `--workspace` selects
      it and both `cargo test` and `--all-targets` then build its lib as a unit test, which is a
      second compilation of the 411,428-line generated file the crate exists to build once. Clippy
      still lints the lib through `RUSTC_WORKSPACE_WRAPPER`, so this loses no coverage, and it is
      what makes carrying `--workspace` from the first commit actually free. Note it changes
      nothing *today*: no CI command passes `--workspace`, and with no `default-members` a bare
      `cargo test` selects the root package alone. It is entirely a prerequisite.

## Phase B: reshape in place, still one crate

Every file ends up in the sub-module of the crate that will own it, while the tree stays one crate
and stays green. **The shape is sub-modules under the directories that already exist**, not new
top-level ones: `services/{net,platform,integrations}/`, `media/{image,ingest,fetch}/`,
`player/{source,engine}/` beside the `output/` that is already the worked example. Phase C's
`git mv src/services/net crates/melodia-net/src` is no less mechanical for it, the crates and the
directories do not nest alike anyway, and it is what keeps every `src/<dir>/**/*.rs` rule glob
matching so only the literal-path ones need editing.

Renumbered into landing order, so nothing moves twice. Each is its own commit, and each ends green
on `cargo clippy --all-targets --locked -- -D warnings` then `cargo test --locked`.

**`test_support_tests::every_path_a_rule_names_still_matches_something` is this phase's automatic
gate.** Nine rules name a `src/services/…` or `src/media/…` path literally, so most items below
turn it red; the fix is the glob edit in the same commit, never a skip. That is A10 working.

- [x] **B0. This pass.** Findings 18 to 21, ten corrections folded into the items below, the graph
      redrawn, and the After-A checks re-run. Docs only.
- [x] **B1. Split monolithic `services/mod.rs`, 285 lines, 7 net against 4 core.** The hard
      prerequisite for the rest: all four media fetchers, `radio_browser`, both scrobble providers,
      `updater/github` and `player/hls/` depend on the net half, and those five `hls/` and
      `stream_source` lines are the whole of what the After-A check still allows out of
      `src/player/`.
      - **Core half to `src/utils/`, as two modules rather than one grab-bag.** `utils/exe.rs`
        takes `current_exe`, `is_dev_build` and the private `undeleted_exe`; `utils/redact.rs`
        takes `redact_home`, `home_dir_string` and the private `redact_prefix`. Roughly 18 call
        sites. **This is what deletes finding 21's `config` to `services` cycle**, and the one
        `super::` spelling of any of the thirteen (`desktop_integration.rs:71`).
      - **Net half to `src/services/net/`**, a directory from the first commit so B6 only adds
        siblings. Roughly 24 call sites, `read_capped` carrying 12 of them. All four
        `build_http_client` uses are `OnceLock::get_or_init` *function references*, so it stays a
        plain `fn` path rather than becoming a method.
      - **The test module splits three ways, and its shared helper decides where.**
        `services/tests/mod_tests.rs` is 733 lines and is two files: eight unit tests plus the
        `current_exe` corpus walk are the core half, two corpus walks are the net half, and the
        remaining ~500 lines (9 tests, 7 helpers, 9 consts) walk packaging, CI workflows, the
        bundled fonts and thread-name length, answering nothing about `services` at all. **The
        three corpus walks share `spellings_outside` (`:148`), which has no dependency on either
        half, so it hoists into `test_support`** rather than being copied twice; that is the leaf
        `melodia-testkit` becomes. The tree walks stay where they are for now under a name that
        says what they are, since finding 8's one-home question is Phase D's.
      - Two exemption tables hard-code `SRC_DIR`-relative paths this item invalidates (`EXEMPT`
        `:117`, `BODY_READ_EXEMPT` `:204`). Both assert their exemptions were *reached*, so a
        stale path fails loudly. `hls/playlist.rs:189`'s explicit intra-doc link alias breaks too.

      **The rules-glob pin stayed green and that is the one thing it could not have caught.**
      `diagnostics.md` names `src/services/mod.rs`, which still exists and now holds nothing but
      module declarations, so the glob matched a file that had stopped carrying what the rule
      describes. A path that moves fails loudly; a path whose *contents* move does not, and the
      only defence is reading the frontmatter of every rule an item touches. It now names
      `src/utils/redact.rs`.

      Nine prose references outside the code went stale with the move and are fixed here rather
      than left for Phase D, four of them prohibitions in the root `CLAUDE.md`: the two corpus
      walks are `utils::exe`'s and `services::net::tests`' now, not `services::tests`'.
- [x] **B2. The three leaf predicates go to core** (finding 21). `AUDIO_EXTENSIONS` and
      `is_audio_extension` from `media/mod.rs` to `utils/audio_ext.rs`, and `media/self_writes.rs`
      to `utils/self_writes.rs`. Five call sites for the first, four for the second. It lands
      beside B1's new `utils/` work and clears three things at once: `single_instance.rs:31`,
      after which the only `crate::` path that file holds points at core rather than at store,
      and the two test-only reads of `AUDIO_EXTENSIONS` from `player/` and `services/`.

      **`library-data.md` is the second rule whose globs had to follow its prose**, for
      B1's reason: it argues both the extension predicate and `SelfWrites`, and named only
      `src/media/**`. The pin cannot see that either, both files having existed all along.
- [x] **B3. `themes/` splits three ways, not two.** The first pass had `apply.rs` as the views half
      whole, and it is not: it also names `crate::Theme` at `:9`, and it carries about 55 lines of
      pure palette computation the graph section already calls palette. `parse_hex_color` (`:94`),
      `palette_from_kde` (`:108`), `LUMA_R/G/B/THRESHOLD` (`:229`) and `on_accent_hex` (`:246`)
      move into `themes/palette.rs`, which today has zero `use` lines, taking
      `themes/tests/apply_tests.rs` with them and ending `registry_tests.rs:6`'s cross-half import.
      What is left is `write_palette`'s 19 brushes, `accent_brushes` and the six brush/colour
      converters, and it goes to `src/ui/appearance/theme_apply.rs` beside its only production
      caller (`ui/appearance/mod.rs:104`). **`themes/mod.rs:34-37` inverts**: the palette half
      stops re-exporting the Slint half upward, and the seven `crate::themes::{brush,color,…}`
      importers under `src/ui/` re-point. `ui/tests/hero_backdrop_tests.rs:490-513`, the walk
      enforcing that `themes::apply(` has exactly one caller, re-anchors with them.

      **Two things the item did not budget for, both deletions.** The kde half went to
      `themes/kde.rs` rather than to `palette.rs`: that module already owns Breeze's static
      tables, and the `kdeglobals` re-source is the dynamic derivation for the same theme, so
      `tests/apply_tests.rs` became `tests/kde_tests.rs` and stopped being a file named after a
      module it no longer describes. And `accent_picker.rs` carried a hand-rolled `brush_from_rgb`
      whose doc said it was duplicated "to keep the themes module's surface minimal" — an argument
      that dies the moment `brush` lands in its own directory, so the copy goes.
      `registry_tests.rs`'s `accent_brushes` test moved with the function it tests.

      Left for B7: `registry_tests.rs` `include_str!`s `melodia-ui/ui/theme.slint` to pin
      `is-light` and `ink-on` against `on_accent_hex`. That is a palette-tier test reading the UI
      tree, the same class as the four B7 already carries.
- [x] **B4. `dwm_titlebar` splits, and the duplicate `win32_hwnd` collapses with it.** Six
      functions, not four: `set_immersive_dark` (`:92`) and `set_caption_color` (`:124`) are the
      actual FFI bodies and take only `*mut c_void` plus a scalar, so they follow `apply` down
      unchanged. The platform half is those two, `is_dark_from_rgb`, and an
      `apply(hwnd, caption_rgb)`; `reapply_from_theme` reads `Theme.mantle` back off the global
      and joins `ui/appearance/theme_apply.rs` from B3. **`win32_hwnd` exists twice**, at `:76` and
      again at `main.rs:500` for souvlaki's SMTC attach, with identical bodies and no import
      relationship. One survives, in `ui/window_chrome/`, serving `theme_apply` and `main.rs:415`
      both: doing the `WinitWindowAccessor` hop once rather than twice is the point of moving it at
      all. The file is **not** `cfg`-gated at the module level (`services/mod.rs:10` declares it
      unconditionally), so check the manifest rather than assuming a `cfg` will catch a mistake.
      Its caller is `main.rs:446`, not `:466`.

      **Everything this item touches is `cfg(target_os = "windows")` and no Windows target is
      installed**, so the local gate compiles none of it and `clippy-windows` / `test-windows` are
      what actually check it. Two things that made that survivable: the two SAFETY comments named
      `win32_hwnd()` as the source of a live handle, which is now a *caller* precondition and says
      so, and `reapply_from_theme`'s inline shift-and-or turned out to be `color_to_rgb`, which it
      now sits beside.
- [x] **B5. `media/` regroups three ways**, into `media/{image,ingest,fetch}/`:
      `image/` is `artwork/`, `cover_thumbs`, `image_decode`, `logo_tile` and
      `services/material_you.rs` pulled in (~2,190 lines, and the only tier with no outbound
      `crate::` edge at all); `ingest/` is `scanner`, `metadata`, `watcher`, `tag_writer` and
      `rating_tags` (~1,180); `fetch/` is `deezer`, `itunes`, `station_logo` and `logo_discovery`
      (~670, every one a caller of B1's net primitives). `self_writes` has already gone to core in
      B2. **`UnsupportedFields` answers its own question**: it is named nowhere outside
      `tag_writer.rs`, so it stays there at no call-site cost. `logo_tile` stays in the image tier
      although `station_logo` is its only consumer anywhere, since net already depends on artwork
      and the alternative puts pixel composition in the crate that owns sockets.
      `media/tests/image_decode_tests.rs:31` pins `EXEMPT = "services/material_you.rs"` as an
      equality, so the `material_you` move breaks it by design.

      **Five corpus pins went red on the move and every one of them was right**: that same
      equality, `lofty_open_tests`' exemption, the thread-name ledger's `cover_thumbs` path,
      `radio.md`'s glob, and the `.slint` comment restating `MIN_LOGO_DIM` under its old module
      path. Four `.slint` files name a Rust module in prose and none of them is walked for, so
      they were found by grep rather than by the suite.

      **The image tier is not edge-free, and the doc's own finding said why without drawing the
      conclusion.** `material_you` names `themes::{Palette, material3}`, so as it stands
      `melodia-artwork` would depend on whichever crate holds the registry. It creates no cycle
      (platform is core-only), and the two answers are to accept the edge or to notice that
      `Palette` is sixteen `u32`s with no behaviour and belongs further down. **Phase C decides
      it**; the module doc carries the question so it cannot be decided by accident.
- [x] **B6. `services/` regroups into net, platform, integrations and app.** The largest item, and
      three of its four groups changed under findings 18 and 19.
      - **`net/`**: B1's primitives, `radio_browser/`, `radio_blocklist/` with its bake left wired
        as it is until Phase C.
      - **`platform/`**: `allocator`, `tray/`, `logging`, `crash_report`, `single_instance` (a leaf
        after B2), `system_theme`, `desktop_integration`, `always_on_top/` with its two `&AppState`
        parameters narrowed to `&Paths` and the cached method, B4's lower half of `dwm_titlebar`,
        and the updater's closed sliver as `install_kind/` (`install_target` folded in from
        `updater/mod.rs`, plus `target`, `linux_pkg`, `probe`, `system_install`).
      - **`integrations/`**: `scrobble/`, `discord/` and **`media_controls/`**, the three that will
        share the crate `load_dotenv` owns.
      - **App remainder, flat at `src/services/`**: `settings/`, `view_state.rs`,
        `search_history.rs`, `artist_images.rs`, `diagnostics.rs`, and `updater/` whole but for the
        sliver. `artist_images` is app rather than net because it orchestrates rather than fetches,
        and `diagnostics` because it names `database` and `state`.
      Five rules name a moved file literally and are edited in this commit: `blake3.md` and
      `updater.md` (`desktop_integration`), `desktop-shell.md` (`tray`, `media_controls`,
      `always_on_top`, `dwm_titlebar`), `diagnostics.md` (`logging`, `crash_report`, and
      `services/mod.rs` from B1), `radio.md` (`radio_browser`). Nine globs in all, plus the radio
      facade pin, whose two path constants both moved.

      **Two platform-to-app edges, not one, and the item budgeted for neither.**
      `always_on_top`'s `&AppState` was the known one and narrowed as planned; its
      `AlwaysOnTopMethod` gained `Copy` on the way, every variant being a unit one.
      `logging::install` was the other: it opened `settings.json` itself to read one bool before
      the logger existed, which is a platform adapter reading the app's file. It takes `verbose`
      as a parameter now and `main` does the read one line earlier, so the ordering the doc
      comment argues for is unchanged. **`src/services/platform/` reaches `config`, `error`,
      `themes` and `utils` and nothing else.**

      **What is left is one class, not two edges, and Phase C should decide it once.**
      `integrations` names `settings::{ScrobbleFlags, DiscordFlags}` at six sites, and B5 left
      `image` naming `themes::Palette`. All three are plain serde or plain-data structs sitting one
      layer above everything that reads them, which is the shape `entities/` already exists for.
      Either the flag structs and `Palette` go there, or three crates grow a dependency each on the
      crate that merely happens to declare them. Deciding it per-edge is how it gets decided three
      different ways.
- [x] **B7. The cross-tier assertions leave the tiers that cannot hold them.** Three tests, not four,
      reach upward across a boundary they will not be able to cross, so all four become integration
      tests under `tests/` where Phase D is taking the corpus walks anyway.
      `media/artwork/tests/artwork_tests.rs:118` needs `ui::grid_prewarm::cover_size` as a
      *function* over a 9x5 sweep, so it cannot reduce to a `const _` assertion, and
      **`STORE_MAX_DIM` is `pub(crate)`** so it widens to `pub` with the move.
      `services/tests/view_state_tests.rs:120` reaches `ui::radio::NAV_RADIO`, an app test naming
      views.

      **The item's list was wrong in both directions and a scan is what found that.** Asking every
      tier's tests which `crate::` roots they name turned up a third the list did not have,
      `database/queries/tests/search_tests.rs:349` reaching `ui::row_match::search_fields`, which
      is store naming views. And two of the four listed are not blockers at all: the
      `dwm_titlebar` oracle stopped being one when B3 put `on_accent_hex` in `themes::palette`
      rather than in views, and `image_decode_tests.rs` walks source *text* through the testkit's
      anchors and names no `ui::` type, so it compiles in a leaf crate and stays where it is until
      Phase D moves the walks together.

      The three land in `tests/cross_tier.rs`. `STORE_MAX_DIM` widens to `pub` as the item
      predicted; nothing else needed to, every other symbol involved already being public.
- [x] **B8. `player/` regroups into `source`, `playback` and `engine`**, so
      the three-way extraction in Phase C is a move rather than a design decision made under a
      compile error. **Verified clean to cut**: zero wrong-direction `use` edges in all three
      directions, A8 having taken the last one (`file_decode.rs` now reads `frames_in`,
      `frames_to_duration` and `interleaved` from `audio.rs`). Tiers measure 3,523 / 4,463 / 3,650
      production lines. Two things it owes beyond the moves. **Six intra-doc links cross a tier
      upward** and would degrade to plain text rather than failing (`audio.rs:6` and `:23`,
      `prebuffer.rs:15`, `crossfade.rs:132` and `:352`, `visualizer.rs:336`); re-word them rather
      than re-pointing them, since pointing up is the whole problem. And **`player/tests/helpers.rs`
      imports the engine tier** (`state::PlayerViewModelLight`, `types::RadioNowPlaying`) while all
      three tiers' tests use it, which needs nothing while the tree is one crate and needs an
      answer in Phase C. `src/player/CLAUDE.md` names about twenty modules by bare filename and
      every one takes its tier prefix; `radio.md` names three of them literally.

      **The middle tier is `playback/` with `output/` nested inside it, not `output/` widened.**
      The item said "the `output` that already exists", but `melodia-playback` is `output/` *plus*
      nine flat DSP files, so leaving `output/` alone buys the nine nothing and leaves the tier
      non-contiguous. `output/` also has a precise meaning the crate does not share — everything
      *under* the DSP chain — and nesting keeps that exact while giving each tier one directory.

      **Thirty-six `super::` paths crossed a tier once the directories existed** and none of them
      is visible as an edge before the move: `super::audio` reads identically whether it resolves
      one directory up or three. They were rewritten by resolving each against its file's real
      module path rather than by hand. The verification is what the split was for:
      `source/` names the network in four files and cpal in none, `playback/` names cpal in eight
      and the network in none, and the two sets do not intersect.

## Phase C: extract the crates

Thirteen steps in **four commits, 4 / 4 / 4 / 1**: C1-C4, C5-C8, C9-C12, then C13 alone. Order
inside a group is forced by the graph, and the compiler does most of the work, nothing building
until a step's `pub(crate)` widenings and facade lines are right. The full gate runs at each commit
boundary.

C13 gets a commit to itself because it is not the same kind of change as the twelve before it. Those
move Rust between compilation units; C13 also relocates `melodia-ui`, rewrites 135 `include_str!`
hops, and drags four non-Rust concerns with it (the rule globs, the testkit anchors, three
`scripts/` helpers and a `.gitignore` rule that fails silently). Reviewing that beside an ordinary
extraction is how one of them gets missed.

**Crates land at `crates/melodia-<name>/`, each keeping the `src/`-relative directory path of the
files it owns** — `melodia-artwork` is `crates/melodia-artwork/src/media/image/**`, not
`crates/melodia-artwork/src/**`. That is not cosmetic. Every intra-crate `crate::` path resolves
unchanged, so a moved file needs edits only where it genuinely crosses a boundary; and every corpus
walk's exemption table keeps its spelling, `crates/melodia-core/src/utils/exe.rs` relativized
against `crates/melodia-core/src` being exactly what it was against `src/`.

The root package stays the binary and a shrinking facade for the whole phase. Phase D moves
`main.rs`, `boot/`, `shutdown.rs` and `tests/` into `crates/melodia/` and makes the root virtual.

### What Phase B left open, decided once

Phase B closed asking for one decision rather than three: `integrations` names
`settings::{ScrobbleFlags, DiscordFlags}`, `media/image` names `themes::Palette`, and all of them
are plain data one layer above every reader. A read of the tree found a third edge of the same
shape and one hard cycle, so the answer is a rule rather than three placements.

**Plain data shared by more than one tier lives in core.**

- **`themes/` goes to core whole**, not to platform, and `KdeColorPalette` moves into `themes/kde.rs`
  with it. The "Why `themes/` splits" section above carries the argument: platform is a cycle, and it
  would put the tray's dependency tree under a pure image crate.
- **`ScrobbleFlags` and `DiscordFlags` go to `entities/`.** They are the only `services::settings`
  items integrations names, at four production sites and two test ones. The on-disk shape is
  unchanged; the parent struct is what flattens them into `settings.json`.

Two further decisions the phase needs and the doc did not carry:

- **A `#[cfg(test)]` fixture cannot cross a crate boundary, so the two that must become
  `#[doc(hidden)] pub`.** `player/tests/helpers.rs` spans all three player tiers *and* has consumers
  in app (`library/tests/playback_tests.rs:493`) and views
  (`ui/now_playing/tests/now_playing_tests.rs:6`). It splits three ways: the pure `f32` helpers name
  no audio type and go to testkit; `TestSource` and `shape` become a fixtures module on
  `melodia-audio`, whose whole argument is that a new source kind is a new `AudioSource`; the three
  engine-typed builders become one on `melodia-engine`. The precedent is `DbPool::test_pool()`,
  `#[doc(hidden)]` and deliberately not `cfg(test)` for this exact reason, and `lto = "fat"` is what
  makes it cost the shipped binary nothing.
- **Finding 2's fix for `melodia-testkit` no longer fits and the leaf property wins.** It assumed
  two call sites where the tree has 24 across three crates. `resolved_home()` is a one-line wrapper
  over `utils::redact::home_dir_string()` and is deleted, its 4 sites calling the production
  function directly — which is what its own doc comment argues for. `paths_in(dir)` is
  `Paths::rooted_at` plus `create_dirs`, and each of the three crates whose tests want a throwaway
  data root keeps its own four-line copy. Four lines of fixture scaffolding in three places is the
  price of testkit naming no workspace type at all.

### The steps

- [x] **C1. Workspace scaffolding.** The `--timings` baseline in Prerequisites, taken here because
      this is the last point at which the "before" is one crate. Every dependency into
      `[workspace.dependencies]` — **all of them, not only the shared ones**: a dep used by one
      member still wants its version and the paragraph arguing it in one place, and a member that
      decides nothing cannot drift. `repository` and `homepage` into `[workspace.package]`, without
      which `ui::settings::about`'s `CARGO_PKG_REPOSITORY` goes empty and only *logs* about it.
      The nine upward intra-doc links in what becomes core, artwork, playback and engine demoted to
      backticked prose, since nothing in the gate runs `cargo doc` (finding 17) and they would
      otherwise degrade silently; `utils/exe.rs:23` was doubly stale, naming a re-export path B6 had
      already moved.

      **`--workspace` onto the CI gate belongs here rather than in Phase D.** With a root package
      present a bare `cargo test --locked` selects that package alone, so every extraction from C2
      on would have silently dropped its own crate's tests from CI. Five commands take it, across
      `pr-validation.yml` and `deploy-coverage.yml`. A11 is what made it free.

      `members` does **not** gain `crates/*` here: cargo errors on a member glob that matches
      nothing, so that line lands with the first crate.
- [x] **C2. `melodia-testkit`.** Ahead of core rather than behind it, the doc's graph order being
      about dependencies where what binds here is the dev-dependency: core's own tests name
      `test_support`, so it cannot leave while the testkit is still inside the package it left.
      36 `pub(crate)` items widen to `pub`, `glob`, `image` and `tempfile` become its own
      dependencies, and Decision 4 removes the two typed helpers first so the crate names no
      workspace type at all.

      **`SRC_DIR` was the design item and it is gone**, replaced by `rust_sources()` over a root
      *list* — `src/` plus each `crates/*/src` — since "every Rust file in the app" is one walk per
      crate now. Paths come back relative to the crate root that produced them, which the layout
      rule keeps unique below the top level, so all ten exemption tables and the 13-entry
      `CALLBACK_HOMES` equality read exactly as they did. Files sitting directly in a crate's
      `src/` take their crate's name in front, every crate otherwise having a `lib.rs`.

      **Four tree-wide rule globs had to gain `crates/**/*.rs` and the pin cannot see why.**
      `code-style.md`, `rust-performance.md`, `tokio.md` and `unsafe-rust.md` all say `src/**/*.rs`
      meaning *all Rust*, and that keeps matching a directory that is emptying out. `unsafe-rust.md`
      is the one that shows it: the sanctioned `set_var` block it exists to govern is now in
      testkit, which its glob had stopped reaching. B1's lesson again — a path that moves fails
      loudly, a path whose contents move does not.

- [x] **C3. `melodia-core`.** `error.rs`, `config.rs`, `entities/`, `utils/`, `themes/`, and
      `src/tests/{config,error}_tests.rs` — the last two reached by `#[path]` from `error.rs` and
      `config.rs`, so they resolve outside every directory a per-directory grep would walk. Carries
      both halves of the decision above, and the four `material3` semantic consts widen to `pub`
      because the palette generator reusing them verbatim is now in another crate.

      **The cycle break landed as the decision predicted and cost one struct.** `KdeColorPalette`
      moved into `themes/kde.rs`, beside the `palette_from_kde` that consumes it and the
      `SystemColorState` that carries it; `system_theme.rs` imports it now rather than declaring
      it, and dropped its `serde::Serialize` with it. `melodia-core`'s manifest names no workspace
      member, which is the property eleven other crates rest on.

- [x] **C4. `melodia-artwork`** — `media/image/`. Core and `slint`, and after the themes decision
      that is the whole production set. Four `pub(crate)` functions widened, all of them the store's
      write side (`store_image`, `cache_image_file`, `compose_artwork`, `compose_cover`), which is
      the interface being stated rather than churn: every one is called from a crate that now has
      to name artwork in its manifest to reach it.

      **Four more rule globs went silently out of reach**, the same shape as C2's. `lofty.md`,
      `rayon.md`, `blake3.md` and `library-data.md` all say `src/media/**/*.rs`, which still matches
      `fetch/` and `ingest/` while no longer reaching the tier that does the BLAKE3 hashing, the
      rayon decode pool and the lofty picture reads. `serde.md` says the same and is the one that
      genuinely does not govern the image tier, so it was left alone.

      > **Commit 1 landed here** (`1060d8f1`). Four crates out, `melodia-testkit` a leaf, and the
      > same 2,237 tests passing across nine binaries rather than six.

- [x] **C5. `melodia-net`** — `services/net/` + `media/fetch/`. `services/net/` alone names only
      core; `media/fetch/` is the entire reason the `net -> artwork` edge exists. Takes
      `radio_blocklist`'s bake into its own `build.rs`, the two `.env.radio.*.local` files staying
      at the repo root through `CARGO_MANIFEST_DIR/../..` so the `gh secret` workflow keeps its
      paths. All seven HTTP primitives widen to `pub`, which is the crate's whole interface.

      **The root `build.rs` keeps `load_dotenv` and would have been wrong to lose it.** Only the
      blocklist bake moved: the `option_env!` sites are still in this package until C11, and
      `cargo:rustc-env` reaches only the crate whose script emitted it, so pulling the dotenv half
      out early would have shipped a keyless build with nothing to say so. The bake's own move is
      checkable and was checked — `melodia-net`'s `OUT_DIR` artifact carries the same term and
      pattern counts the root one did.
- [x] **C6. `melodia-platform`** — `services/platform/`. Core alone, and `cargo tree` says so.
      Nine `include_str!` hops gain two `../` each, four of them production: the `.desktop`
      template, the two icons and the `MetaInfo` XML.
- [x] **C7. `melodia-audio`** — `player/source/`. Core plus `melodia-net`'s four functions.
      A8's three numeric helpers (`frames_in`, `frames_to_duration`, `interleaved`) widen to `pub`,
      which is that item paying off: they were the one wrong-direction edge inside `player/`, and
      moving them into `audio.rs` is what left this a manifest line rather than a refactor.
- [x] **C8. `melodia-playback`** — `player/playback/`. Core plus audio.

      **Decision 3 was right about the problem and too expensive about the fix.** A read of the
      actual usage narrowed it: `TestSource` and the float helpers are read by *playback alone*,
      so they stayed `cfg(test)` beside its tests rather than becoming a public fixture on audio;
      audio's only use of the shared file was `shape`, so it keeps a five-line `cfg(test)` copy of
      the three `NonZero` constructors. Only the transport trio genuinely crosses — engine's own
      suites, `library`'s and the now-playing ladder's under `ui/` — and only that became
      `#[doc(hidden)] pub`, as `player::engine::fixtures`. One production export instead of two,
      and the one that survived is the one with content: `RadioNowPlaying` has thirteen fields and
      `PlayerViewModelLight` thirteen more, where a `Shape` constructor is five lines.

      > **Commit 2 landed here** (`4c8ad257`). Eight crates out; `melodia-audio` names no cpal and
      > `melodia-playback` no reqwest, both now enforced by cargo rather than asserted by a grep.
      > 2,237 tests across thirteen binaries.

- [x] **C9. `melodia-engine`** — `player/engine/`. Core, audio and playback, and nothing else; the
      cut cost no widening at all, B8 having already resolved every wrong-direction edge.

      **`src/player/CLAUDE.md` had to move, and it had already stopped working.** A nested
      `CLAUDE.md` loads for its own directory subtree, so once C7 and C8 took `source/` and
      `playback/` to `crates/`, the contract doc for two of its three tiers was reaching nothing —
      the rule-glob failure in `CLAUDE.md` form, and invisible because A10's pin only reads
      `.claude/rules/`. It is now `.claude/rules/audio-stack.md`, globbed over all three tiers plus
      the `src/player/mod.rs` facade, which is the shape the root `CLAUDE.md` already prescribes for
      a subject spanning trees no one directory reaches. Six live references followed it.
- [x] **C10. `melodia-store`** — `database/` + `media/ingest/`. Carries `sqlx::migrate!` at
      `database/mod.rs:214` and `:313`; note the second is inside `test_pool()`, which is
      `#[doc(hidden)]` rather than `cfg(test)` and so compiles into release. `migrations/` stays at
      the repo root, being shipped and checksummed, so the macro argument becomes the hop to it.

      **Decision 3's list of two fixtures is three.** `database/queries/tests/helpers.rs` was
      `#[cfg(test)]` with four consumers in what became `melodia-app` —
      `library/tests/{tags,mbid}_tests.rs`, `library/playlist_files/tests/` and
      `tasks/tests/reconcile_tests.rs`, all seeding through `insert_test_track` — so it became
      `queries::fixtures`, `#[doc(hidden)] pub`, on `DbPool::test_pool`'s precedent and for its
      reason. Eighteen imports re-point. Six `pub(crate)` items widened, every one named by rustc.
- [x] **C11. `melodia-integrations`** — `services/integrations/`. Takes `load_dotenv()` into its own
      `build.rs`, reading `.env` from the repo root. Finding 5's hard constraint, and the one whose
      failure is invisible: CI passes the keys as environment variables and the environment wins, so
      only a local build would ever notice a missed hop.

      **Verified by evidence rather than by reading**, which is what the invisibility demands:
      `melodia-integrations`' build-script `output` carries both key names and the root package's
      carries neither, and the key value appears in the compiled `libmelodia_integrations` rlib.
      The move itself was the cheapest of the four — **every app-ward edge in the directory was
      prose**, five intra-doc links and not one call, so the crate needed no narrowing to fit under
      core, net and engine.
- [x] **C12. `melodia-app`.** `library/`, `tasks/`, `state/`, and the flat remainder of `services/`.
      Two literals break and neither is a `crate::` path: `library/tests/radio_tests.rs:17`
      hard-codes `src/library/radio` in a `concat!`, and `services/tests/view_state_tests.rs:123`
      `include_str!`s a bin file, joining the cross-tier pins B7 moved to `tests/`.

      **Finding 13 is enforced by visibility rather than by omission, and it is checkable.** App
      names every crate below it and views will name app, so the four paths views may not reach —
      `database`, `media::{ingest,fetch}`, `services::net`, plus `player::source` — are
      `pub(crate) use` in its facade. Deleting nothing and merely *trying* the import from the root
      package fails with `module database is private`, naming the facade line. The root package
      keeps its own path dependency on store and net for `src/lib.rs`'s three shim modules, which is
      what `boot/` and `tests/` reach through.

      **The root manifest carried fourteen dependencies no file in it named any more**, left behind
      by commits 1 and 2 — `symphonia`, `realfft`, `flexi_logger`, `interprocess`, `zbus`, `ksni`,
      `libc`, `tray-icon` and the rest — plus a `[build-dependencies] blake3` dead since C5. Nothing
      in the gate can see that (`unused_crate_dependencies` is allow-by-default and outside the
      `unused` group), so the binary was still compiling and linking the whole lower half of the
      graph. Pruned here, `sqlx` demoted to a dev-dependency, and the eleven `melodia-*` path lines
      are what remain of the topology at the top.

      > **Commit 3 landed here** (`8a38451a`). Twelve crates out; `melodia-store` names no socket and
      > `melodia-integrations` no schema, both now cargo's answer rather than a grep's. The same
      > 2,237 tests across seventeen binaries.

- [x] **C13. `melodia-views`, with the `melodia-ui` relocation.** Its own commit, for the reason
      given above. `src/ui/` and `melodia-ui/` move in
      the same commit, because the `include_str!` hops into `melodia-ui/ui/` are relative paths
      whose depth changes with *either* move; together they are rewritten once rather than twice.
      **131 of them, in 31 files, all +1** — the file goes two levels deeper and the target one, so
      the delta is uniform whatever the file's depth. The other 91 `include_str!` under `src/ui/`
      read a *sibling* `.rs` and needed nothing, both endpoints moving together. That pulls the
      `melodia-ui` half of Phase D forward: its 43 rule globs, the `UI_DIR` / `UI_SRC_DIR` /
      `FONTS_DIR` anchors, `locale_tests.rs:22`'s hard-coded `translations` path, the `scripts/`
      font and icon helpers — **four, not three: `gen-discord-assets.sh` writes into the icon tree
      too** — and `.gitignore`'s anchored `/melodia-ui/ui/assets/fonts/originals/`, which would
      otherwise stop working silently and make the pristine faces committable.

      **The crate-rooted `include_str!` sites are four rather than three, and the fourth is the
      one a grep misses**: `entities/tests/smart_criteria_tests.rs` puts the macro on one line and
      its path literal on the next. All four lose a `../`, the opposite sign from the 131 inside
      views, which is the shape of mistake a single mechanical pass makes.

      **No `pub(crate)` widened, and the rolling-work paragraph is what predicted otherwise.**
      `main.rs`, `boot/` and `tests/` are already separate crates from the lib and reach views as
      `use melodia::{… ui}`, so everything they touch was `pub` before the cut; the 80 production
      `pub(crate)` under `src/ui/` are all intra-directory and the 350 `pub(super)` are unaffected
      by a subtree that moves whole. C13 is the one step cheaper than the plan.

      **Views names each crate it reaches rather than re-exporting `melodia_app`'s shims**, which
      would have compiled — app's facade exposes exactly `media::image` and `player::{engine,
      playback}` — and would have left five of the eight edges out of the manifest, making the
      After-C check vacuous. Under the facade form, deleting `melodia-artwork` from views produces
      no error at all. The eight are core, artwork, platform, playback, engine, integrations, app
      and `melodia-ui`; store and net are the absence the check reads.

      **`crate::services::diagnostics` was missed by the survey and caught by rustc**, because the
      views shim enumerates sub-paths and two of its callers spell the bare `services::` form after
      a `use crate::services;`. The same trap holds `library::` at 330 lines against 98 for
      `crate::library`, which cost nothing only because that one is a whole-module re-export.

      **The root package loses `melodia-net` and eleven third-party dependencies.** Nothing left
      in it opens a socket, so `services::net` and `media::fetch` came off the shims and the
      manifest line with them; `tokio-util`, `parking_lot`, `rand`, `unicode-normalization`,
      `image`, `lru`, `material-colors`, `chrono`, `rfd`, `open` and `futures-util` all went to
      views. What the binary still spells is `slint`, `tokio`, `log` and `async-compat`.

      **Two things nothing in the gate could have caught.** `rust_source_roots()` enumerates
      `crates/*/src`, so moving `melodia-ui` under `crates/` silently enrolled its `lib.rs` in all
      eleven corpus walks — benign, and verified by running them rather than by reading it. And
      `packaging/debian-copyright`'s four DEP-5 `Files:` stanzas name the font paths while its pin
      checks only the header and the quoted licence bodies, so those would have gone stale with
      nothing to say so and left the bundled fonts under the package's blanket AGPL declaration.
      `licenses/ATTRIBUTION.txt` is the same paths and *is* pinned, on `rel_path(REPO_ROOT, …)`.

      **The four `env!` package reads under `src/ui/` are the silent regression finding 3
      predicted**, and the manifest is what answers them: `CARGO_PKG_VERSION` at three updater
      sites and `CARGO_PKG_REPOSITORY` at `ui/settings/about.rs:18` go to whatever crate the files
      land in. Checked by evidence rather than by reading — `libmelodia_views.rlib` carries both
      the repository URL and `0.12.0`.

      23 `.slint` prose comments named `src/ui/…` and five more were already stale from C3 and
      C12, none of them walked for; B5 met this class and it has not stopped being true.

      > **Commit 4 landed here** (`01c314a8`), and with it Phase C. Thirteen crates; the UI layer
      > names neither the schema nor a socket, which is cargo's answer rather than a grep's and
      > was the exercise. The same 2,237 tests across eighteen binaries.

Rolling work, fixed in the step that surfaces it rather than batched: `pub(crate)` widening where an
item crosses its new boundary (181 production candidates, every one named by rustc);
`[lints] workspace = true` and `[lib] doctest = false` on each member; the relative `include_str!`
hops, each moved file gaining one `../`; and the rule globs, against A10's pin, whose own doc
comment is stale besides (165 entries / 96 literal, against 169 / 99 today).

**Landing tactic for the 2,721 `crate::` paths.** Each consuming crate re-exports what it took
(`pub use melodia_core::{error, config, entities, themes, utils};` in its `lib.rs`), with a nested
shim where a directory spans crates, so `crate::error::AppError` keeps resolving and the diff stays
about topology rather than import churn. Enforcement is unaffected for the crates a member does not
name at all: a re-export cannot reach a crate absent from the manifest. It *is* affected the other
way, so finding 13's rule applies from the first re-export and **every facade is an explicit item
list, never a glob**: never `pub use` a type out of a crate your dependents are meant to be unable
to reach. De-facade in **Phase E** once the graph is proven, one crate at a time.

## Phase D: make the repo workspace-native

Seven commits, `37cad90f` to `8cbbe055`. Everything below is done; the last two are a count the
first five got wrong and the tidy crate folded back in.

- [x] **D1. The binary becomes `crates/melodia`; the root goes virtual.** `src/`, `build.rs` and the
      four integration tests move; the package is `melodia` and `[[bin]] name = "Melodia"` keeps the
      artifact name every download URL, `.desktop` Exec= line and MSI registry key already spells.
      `path` is required beside it — cargo only infers `src/main.rs` for a bin named after its
      package — and `[lib] name` becomes deletable, having existed only to lowercase `Melodia`.

      **A virtual root infers no resolver from its members**, which the plan did not carry: with a
      `[package]` here, edition 2024 implied resolver 3, and dropping that table fell back to
      resolver 1 silently. `resolver = "3"` is spelled out.

      **`[package.metadata.deb]`'s ninth path must not gain `../../`.** `target/release/` is a magic
      literal cargo-deb strips before joining any root and then resolves against the real target
      dir; the other eight and `license-file` are package-root-relative and do gain it. Read out of
      cargo-deb 3.7.0's own source rather than assumed, and checked with a real `cargo deb -p melodia`.

      `tests/assets/` is `test-assets/` at the root — three crates read it through one `ASSETS_DIR`,
      so it cannot sit in one member's `tests/`. `tests/fixtures/` stays the binary's own and keeps
      its own directory: `headless` scans it and counts what lands, so holding exactly one file is
      the assertion rather than an accident, which the plan's "merge them" line would have broken.

      Two pins went red and both were right: `LICENSE_SHIPPERS` read the root manifest for the deb
      asset glob, and the rules pin named all 22 root-anchored globs at once. **`.gitattributes` had
      been stale since C12 with nothing to say so**, pinning the signed updater fixtures `-text
      -merge` at a path that moved to `melodia-app` — the one file in the tree no test reads.
- [x] **D2. Packaging and CI name the package.** `-p melodia` on the release build, the deb and the
      RPM script's `--build` hook; `--package melodia` on the MSI. Without it `cargo deb` does not
      merely build too much, it stops: `Cargo.toml is a workspace, not a package`.

      **`wix/` moves under the binary rather than staying at the root behind `-I`.** cargo-wix reads
      `wix/main.wxs` relative to the manifest `--package` selects, and the flag's resolution base is
      undocumented and exercised by nothing short of a tagged Windows release. Its default layout
      costs three `..` in `RepoRoot` and re-points two pins the local suite runs.
- [x] **D3. The repo-wide checks get one home: `crates/melodia/tests/`.** Finding 6 put `tests/`
      there and finding 8 asked for one home for the walks; this is both, and the two land in one
      directory because the binary is the only crate that is above all of them.

      **A dedicated `melodia-tidy` crate was built first and then folded back in**, which is worth
      recording because the argument for it was thinner than it looked. rustc and rust-analyzer do
      keep repo-wide checks in a crate of their own, but both are *binaries* — `src/tools/tidy` run
      by `x.py test tidy`, `xtask/src/tidy.rs` run by `cargo xtask tidy` — so the precedent supports
      the separate crate and not the shape built, which was integration tests. What the separate
      crate genuinely bought was that a broken app cannot hide a repo check; what it cost was a
      fifteenth member and a `lib.rs` holding nothing but a doc comment. Against a gate that is
      `cargo test --workspace`, where every member is compiled anyway, that trade did not pay.

      **The criterion is the corpus, not the subject**: a check that *enumerates* one moves, a pin
      on one named file stays beside the module it describes. Both halves of the radio off-switch
      pin land together, which is what finding 8 wanted and neither crate could give.

      **Nine self-exemptions are deleted rather than re-pointed.** A walk inside the corpus it walks
      is its own first hit, so each named itself, and two had bent their shape around it —
      `error_as_string` split its needle as `concat!("Result", "<")` and `cover_generation` skipped
      itself while asserting it still spelled what it greps for.
- [x] **D4. The Slint-tree walks follow.** Twenty-four more, reading `crates/melodia-ui/ui` from
      inside `melodia-views`; `melodia-ui` cannot host them, `[lib] test = false` being A11.
      `depth_between` goes to the testkit, being needed on both sides of the split and the same
      category as `blocks_named` beside it. **`SUPPORTED_LOCALES` goes to `entities::locale`** on
      Phase C's own rule, which it meets on its own terms — app validates a persisted code and views
      indexes its native names, two tiers above the crate that merely declared it, which is the same
      shape as that phase's `ScrobbleFlags` call.

      The criterion is exhaustive rather than aspirational: no crate's `src/` calls a corpus
      enumerator at all. Fifty-four checks in twenty-four files.
- [x] **D5. Globs, docs and the gate.** The module map leads with the crate that owns each bullet;
      the four `src/`-era "walks `src/`" claims, the two command blocks missing `--workspace`, and
      twenty references naming a moved pin by its old module path. `CLAUDE.md` gains the tidy rule
      beside its testing conventions.

**Counts the plan carried that the tree did not.** Rule entries 182, not 165 — the pin's own doc
comment said 165/96 and is a floor now rather than a census, which is prose that needed rewriting
on every rule added. Intra-doc links 147, not 162. Test binaries 18 at the start of Phase D and 42
at its end, against the "roughly fourteen" predicted and the "five" `pr-validation.yml` claimed.
The scope-clippy-to-one-crate convention was never in the repo: what was there is two command
blocks omitting `--workspace`, and a virtual root retires the question by making every-member the
default selection.

**The one measurement left open is the CI test job's memory cap.** `CARGO_BUILD_JOBS: 4` is argued
against the number of test binaries linked, and that number is 42 rather than the five its comment
claimed or the "roughly fourteen" this plan predicted — collecting the walks turned 18 into 42, each
walk file being a binary of its own. The count is corrected in the comment; whether 4 is still the
right cap is a question only a runner can answer, since what it holds down is peak RSS during
linking and this machine has more memory than one. What can be said from here is that the walks are
not what would strain it: relinking all of them costs under a second at roughly 400 MB. So leave the
cap where it is until a run says otherwise — too high fails as a swap that looks like a hang, which
is the expensive direction to guess at.

## Phase E: de-facade

Each crate `pub use`s what it took, so `crate::error::AppError` still resolves everywhere and the
Phase C diffs stayed about topology. That was the migration tactic and it has a stopping point.
rust-analyzer's style guide is the shortest statement of why: *"By default, avoid re-exports.
Rationale: for non-library code, re-exports introduce two ways to use something and allow for
inconsistency."* The stronger argument is this repo's own — the split exists so the compiler
enforces topology, and `crate::error::AppError` inside `melodia-views` hides the very layering it
was drawn to expose, where `melodia_core::error::AppError` shows it at the import.

**Ten facade sites, and two of them are not a `lib.rs`**, which is what the After-E grep as
written would have missed: `melodia-app/src/services/mod.rs` re-exports the three adapter groups,
and `crates/melodia/src/{lib,media/mod,player/mod,services/mod}.rs` is a lib target holding
nothing else. Measured at `bede9c69`, roughly 1,850 lines across 540 files carry a facade-covered
path, four fifths of them in a `use` line, so the longer spelling lands at the import and bodies
keep reading `platform::single_instance::…`. Nothing is flattened: every crate holds the
`src/`-relative subtree it owned as a monolith, which is what ten corpus-walk exemption tables and
`rust_sources()`'s own contract rest on.

**Method**, per crate, with the previous crate already committed so there is a checkpoint: delete
the facade lines and the `//!` paragraph arguing them, bulk-rewrite that crate's covered prefixes
with `sed` scoped to its `src/`, then let `cargo clippy -p <crate> --all-targets` enumerate what
the pass could not reach (grouped `use crate::{…}` imports, and bare `services::platform::…`
bodies sitting under a `use crate::services;`), then `cargo fmt --all` for the reflow the longer
paths force. Validation is the compile plus a read of the diff, never the rewrite's exit code.
**`$crate::` is protected before the pass and restored after**: five sites inside `macro_rules!`
bodies would otherwise become `$melodia_core::…`, an undefined metavariable. All five are
`pub(crate) use`d and expand only inside views, so each takes the absolute path with no `$`, which
names the crate defining the item rather than the crate defining the macro.

**`pub use melodia_ui::*` goes with the rest, and the research is what decided it.** The glob
launders no layering, `melodia-ui` being generated output rather than a tier, and the root
`CLAUDE.md` documented it as deliberate, so keeping it was the reading on the table. Three things
against. rust-analyzer's guide, beside the sentence above, also says *"Qualify items from `hir` and
`ast` modules rather than importing them directly. Rationale: avoids name clashes, makes the layer
clear at a glance"*, which is this phase's argument in its positive form and carves out nothing for
generated code. Every generated-code precedent names the generating module at the use site
(prost/tonic's `pb::`, the `windows` crate, diesel's `schema.rs`); the one glob shape the ecosystem
sanctions is a curated prelude, which seventy Slint types are not. And the middle option is not
available: clippy's `wildcard_imports` returns early only where the glob's visibility is *not*
`Restricted(parent_module)`, so `pub use x::*` is skipped while `pub(crate) use x::*` at a crate
root is linted, and pedantic is denied here. Views declares no PascalCase item at its crate root,
so `crate::<PascalCase>` is exactly the generated set and separates cleanly from `crate::ui`.

**The binary's lib target is deleted rather than emptied**, being facade end to end. `main.rs`,
`boot/`, `shutdown.rs` and the integration tests are already a separate crate reaching it as
`use melodia::{…}` and name the owning crates instead; `[lib]` comes off the manifest, and
`melodia-audio` and `melodia-playback` demote to dev-dependencies, nothing outside `tests/`
naming either once the shims are gone. That is one fewer test binary, 41 rather than 42.

- [x] **E0. This section.** Docs only.
- [x] **E1. `melodia-artwork`**, 4 lines.
- [ ] **E2. `melodia-net`**, 13. Also `crate::media::image`.
- [ ] **E3. `melodia-platform`**, 13.
- [ ] **E4. `melodia-audio`**, 11. Its `pub mod services { }` shim empties and goes with the line.
- [ ] **E5. `melodia-playback`**, 21.
- [ ] **E6. `melodia-engine`**, 22.
- [ ] **E7. `melodia-store`**, 48. `crate::database` stays, being store's own.
- [ ] **E8. `melodia-integrations`**, 26.
- [ ] **E9. `melodia-app`**, roughly 157, plus the four `pub(crate) use` enforcement lines and
      `services/mod.rs`'s three. With call sites naming `melodia_store::database` directly the
      manifest is the enforcement, and views not listing `melodia-store` is a harder error than a
      private module: the import does not resolve at all rather than resolving to something
      private. The After-C check is restated in that form.
- [ ] **E10. `melodia-views`**, roughly 321 layering lines.
- [ ] **E11. `melodia-views`, the generated types.** Roughly 224 lines across 174 files, 157 of
      them grouped imports translating one for one to `use melodia_ui::{AppWindow, Settings};`.
      Its own commit because it answers a different question from E10 and reviews separately.
- [ ] **E12. `melodia` (bin).** The lib target and the three shim directories go; `use melodia::{…}`
      is rewritten across `main.rs`, `shutdown.rs`, `boot/` and the integration tests.
      `.claude/rules/audio-stack.md` loses its `crates/melodia/src/player/mod.rs` glob, which A10's
      pin demands in the same commit, and nothing is lost with the file: its `//!` argues the three
      tiers, which is the argument that rule already carries.
- [ ] **E13. The testkit alias.** `crate::test_support` becomes `melodia_testkit` across nine
      crates, roughly 63 sites. It is `#[cfg(test)] pub(crate) use melodia_testkit as test_support`
      and launders nothing, but it is the last place one crate wears two names, and the 42
      integration tests already spell the second.
- [ ] **E14. Docs.** Four claims in `CLAUDE.md` stop being true in their current form: the store
      bullet's `pub(crate)`-in-app's-facade half, the `melodia-ui` bullet's flat re-export sentence,
      the media bullet's "no outbound `crate::` edge" (which goes trivially true and stops meaning
      anything), and the services bullet's `crate::state`/`crate::player` prohibition, which is a
      manifest now. Plus one `crate::` path in `.claude/rules/library-data.md`. The 146 intra-doc
      links naming a `crate::` path need no demotion: the cross-crate ones resolve, every target
      being a declared dependency.

## Verification

Per commit. **All three carry `--workspace` from Phase A onward**, which A11 is what made free;
the plan to grow it only in Phase D is spent:

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked --workspace -- -D warnings
cargo test --locked --workspace
```

Phase boundaries:

- After **A** — **all passing as of `a1c087e4`**:
  `grep -rn 'crate::database\|crate::tasks' src/player/` returns nothing, and
  `grep -rn 'crate::services' src/player/` returns only the HTTP primitives. `grep -rn 'crate::ui' src/tasks/`
  returns nothing. `grep -rn 'crate::media' src/ui/` returns only the image tier.
  Two the list did not name and Phase A also bought: `grep -rn 'crate::ui' src/state/` returns
  nothing (A7) and `grep -rn 'crate::player' src/entities/` returns nothing (A1). The residue in
  the first two is prose — an intra-doc link to `crate::tasks::audio_health`, and two comments that
  name `DbPool` to say why the engine holds none — which finding 17's `cargo doc` gap covers.
  **Re-run before Phase B's first commit and all six still passed.**
- After **B** — **all passing**. The After-A set again, plus one grep per boundary the phase draws. Each is
  `grep -rn`, each must return nothing but prose:
  `crate::services::\(is_dev_build\|current_exe\|redact_home\|home_dir_string\)` over `src/` (B1);
  `crate::` over `src/services/single_instance.rs`, which ends the phase naming none (B2);
  `slint\|AppWindow\|crate::Theme` over `src/themes/` (B3); `fn win32_hwnd` over `src/`, which must
  count **one** (B4); `crate::services\|crate::player` over `src/media/image/` (B5);
  `crate::state\|crate::player` over `src/services/platform/` and
  `crate::services::net` over `src/services/{platform,integrations}/` (B6); and
  `use super::\(output\|state\|backend\|decks\)` over `src/player/source/` (B8).
  Nothing here is measured: no release build, no `/usr/bin/time -v`. The phase moves modules and
  changes no behaviour, and a module boundary costs the binary nothing.
  Two flagship checks now answer in one line each: `crate::media` in `src/ui/` returns
  `crate::media::image` and nothing else, and `crate::services` in `src/player/` returns four
  `services::net::` primitives and nothing else. **What Phase C inherited** was one open question
  rather than a list: `integrations` names `settings::{ScrobbleFlags, DiscordFlags}`, `media/image`
  names `themes::Palette`, and all of them are plain data one layer above every reader, which is
  what `entities/` is for. Answered at the head of Phase C, as one rule and not three placements —
  and the read that answered it found a third edge of the same shape and a hard cycle besides.
- After **C** — **all passing as of C13**. `cargo tree -p melodia-views --depth 1` lists eight
  members and neither `melodia-store` nor `melodia-net`; `melodia-audio` names no `cpal` and
  `melodia-platform` no `melodia-ui`, both over the whole tree rather than the first level. And the
  exclusion errors rather than merely being absent: adding `pub use melodia_app::database;` and
  `pub use melodia_app::services::net;` to views fails with `module database is private` and
  `module net is private`, each naming the `melodia-app` facade line that seals it. Those are the
  flagship rules turned into compile errors, and they are the whole point of the exercise.
- After **D** — **all passing**. `cargo metadata` reports no root package and a `melodia` member
  whose bin target is still `Melodia`, which `--version` confirms by printing `Melodia 0.12.0`;
  `cargo deb -p melodia` resolves `target/release/Melodia` to the *workspace* target dir and the
  other eight assets off the new manifest dir. `grep -rn 'rust_sources()\|slint_sources()\|
  spellings_outside(' crates/*/src` returns nothing outside `melodia-testkit`. The same 2,237 tests
  pass, across 42 binaries rather than 18.

  Left for the release gate, which needs a release build: `scripts/build-{rpm,appimage,tarball}.sh`
  each producing an artifact, `cargo build --timings` against the prerequisite baseline, and
  `/usr/bin/time -v target/release/Melodia` for peak RSS. No RSS change is expected, `lto = "fat"`
  with `codegen-units = 1` recovering cross-crate inlining.
- After **E**: `grep -rn 'use melodia_' crates/*/src` returns ordinary imports and nothing else, no
  `pub use`, no `pub(crate) use` and no glob. **The `crates/*/src/lib.rs` form this line used to
  carry was too narrow**: two of the ten facade sites are not a `lib.rs`, being
  `melodia-app/src/services/mod.rs` and the binary's three shim modules. And the After-C exclusion
  holds harder rather than merely still holding. `cargo tree -p melodia-views --depth 1` lists
  neither `melodia-store` nor `melodia-net`, and where the check used to be that adding
  `pub use melodia_app::database;` to views fails with `module database is private`, naming the
  facade line that sealed it, it is now that `melodia_store::database` names no crate views can
  reach. Both halves are worth spelling out, the second being the whole reason the four
  `pub(crate) use` lines were safe to delete. `melodia-audio` still names no `cpal` and
  `melodia-platform` no `melodia-ui`.

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
