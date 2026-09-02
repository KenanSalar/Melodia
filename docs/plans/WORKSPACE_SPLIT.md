# Splitting `src/` into a workspace

Working doc. Validation of the issue, the dependency graph as the code actually has it, and the
order the cuts come out in. Harvest into `docs/adr/` when
[#84](https://github.com/KenanSalar/Melodia/issues/84) ships, not before: the boundary rationale
below is exactly what #84 exists to stop evaporating.

Status: **not started** · Issue:
[#83](https://github.com/KenanSalar/Melodia/issues/83) · Created: 2026-09-03 · Validated against
`93b47dfa`

> The issue body carries the argument for *why* nine crates. This doc carries what a read of the
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

All five cycles in the issue are real code edges. Six of its numbers are not, and three of its
dependency edges are missing. Both matter: the counts decide which cut is cheapest, and the missing
edges decide the shape of the graph.

| issue says | tree says |
|---|---|
| `Track::replaygain()` | `TrackSummary::replaygain()`, `entities/track.rs:73-82` |
| `state/mod.rs:133,137`, 2 sites | `:151,155,277,278,280`, 5 code lines |
| `ui` reaches `state` at 135 | 138, all production |
| `ui` reaches `tasks` at 27 | 29, and 25 of them are one function |
| `player` reaches `services` at ~6 | 12, and `player/hls/` postdates the issue entirely |
| `ui` reaches `media` at 34, 26 `cover_thumbs` | 28 / 22 non-test; 34 was the all-files count |

Three edges the table omits, each a real call and not a doc link:

- `media/metadata.rs:248` calls `player::file_decode::probe_duration`, so **store depends on audio**.
  It is the sole edge `media/` has into that directory, which `src/player/CLAUDE.md` already says.
- `station_logo.rs:140` and `deezer.rs:245` call `artwork::store_image`, so **net depends on artwork**.
- `scanner.rs:52` and `metadata.rs:178` take `&artwork::CoverCache`, so **store depends on artwork**.

Four files outside `src/ui/` name `crate::AppWindow`, not the two the issue lists: `themes/apply.rs:8`,
`tasks/updater_daily.rs:47`, `tasks/rss_sampler.rs:47` and `services/dwm_titlebar.rs:24`
(Windows-gated). App and platform therefore carry `melodia-ui` whatever happens to `src/ui/`.

And the three-way `media/` split has a hole: `mod.rs` declares 14 modules and the issue's tiers name
13. `rating_tags.rs` is unassigned and belongs with ingest. Going the other way,
`services/material_you.rs` is an image file misfiled under services (`slint`, `image_decode::decode_capped`,
`themes`) and should join the image tier.

## Findings the issue does not carry

Ranked by how quietly they fail.

1. **`version = "0.0.0"` on internal crates would ship a bug.** Ten non-binary sites read
   `env!("CARGO_PKG_VERSION")`: the updater's compare (`tasks/updater_daily.rs:137`,
   `ui/callbacks/updater/check.rs:33`), the Settings version label (`ui/settings/updater_settings.rs:39`),
   the HTTP user agent (`services/mod.rs:89`), crash reports, and ListenBrainz's submitted client
   version. Worse, `repository` and `homepage` sit on `[package]` rather than `[workspace.package]`,
   so `ui/settings/about.rs:18`'s `CARGO_PKG_REPOSITORY` resolves empty and the About page's
   repository button silently no-ops, which that file already logs and returns for.
   **Every member takes `version.workspace = true`, and `repository`/`homepage` move into
   `[workspace.package]`.** `melodia-ui` already sets the precedent.
2. **About 134 of 225 `.claude/rules` path globs stop matching, and the pin cannot see it.**
   `src/tests/test_support_tests.rs:89` does `if glob.contains('*') { continue; }`, deliberately,
   because a glob may legitimately describe an empty tree. So every `src/**/*.rs` entry goes green
   while matching nothing, and `code-style.md` and `tokio.md` stop loading for all Rust in the tree.
   This is the largest silent failure in the change, and the fix is part of Phase D rather than an
   afterthought.
3. **A member missing `[lints] workspace = true` leaves the gate with no CI signal.** It drops
   `unwrap_used`, `unsafe_code`, `await_holding_lock` and all of pedantic, and
   `cargo clippy --all-targets -- -D warnings` reports zero warnings for a crate with no lint table.
4. **Build scripts multiply.** `cargo:rustc-env` and `OUT_DIR` are per-crate, which the root
   `CLAUDE.md` already warns about. Whichever crate holds `scrobble`/`discord` needs `load_dotenv()`
   or the build silently ships keyless and ListenBrainz-only; whichever holds `radio_blocklist` needs
   the bake its `include!` reads back out of `OUT_DIR`. `build.rs`'s `read_to_string(".env")` is
   CWD-relative, and cargo sets CWD to the package root.
5. **Compile-time path breaks. Loud, but numerous.** 241 relative `include_str!` sites across 46
   files, 135 of them reaching into `melodia-ui/ui/`, each hop count a function of that file's depth
   from the package root. Then `sqlx::migrate!("./migrations")` at `database/mod.rs:214` and `:313`,
   `services/updater/minisign.rs:35`'s `assets/updater-pubkey.b64`, `library/tests/radio_tests.rs:17`,
   and `ui/settings/tests/locale_tests.rs:22`.
6. **`[package.metadata.deb]`'s eight asset paths and its `license-file` are package-root-relative**,
   and `target/` stays at the virtual root. Under a virtual manifest a bare `cargo build` and
   `cargo deb` both build every member, so `release-build.yml:83` and `:124` need `-p`.
7. **21 test files anchor on a tree root, 10 of them on `SRC_DIR`.** Walks such as the `rfd` pin and
   the single-resampler equality would keep passing while checking one crate out of nine. `SRC_DIR`
   needs one workspace-root definition, not a per-crate one.
8. **`toast` and `play_count_flusher` are the same primitive written twice**: a
   `OnceLock<UnboundedSender<E>>` plus a plain enum, producer half dependency-free, consumer half
   owning the I/O. `services/toast.rs:10` already notices the resemblance in prose.
9. **`player` names `DbPool` only to serve a test fallback.** The direct UPDATE at
   `actions.rs:107-127` exists because the flusher is not installed in test contexts, and the comment
   there says exactly that. Install it and `db: &DbPool` leaves `execute_actions` and
   `emit_and_execute`, taking `crate::database::{DbPool, queries}` out of the engine with it. That is
   a deletion rather than the new `PlayerAction` variants the issue proposes.
10. **Stale doc comment** at `tasks/rss_sampler.rs:18` names an `ui::window_chrome::is_queue_sheet_open`
    import that no longer exists anywhere in `src/tasks/`.

**Survives untouched**, verified, so no work is planned for it: the four `[workspace.package]`
version scrapes (they anchor on `^\[`, so the `[package]` table vanishing is fine),
`pr-validation.yml`'s `changes` filter (a denylist rooted at `'**'`, so `crates/**` falls through
correctly), every `$REPO_ROOT`-based path in the packaging scripts, and `wix/main.wxs`'s
`$(sys.SOURCEFILEDIR)..`.

## The graph, corrected

```
melodia-core      error, config, entities, utils, describe, atomic json/text writers
melodia-artwork   artwork/, cover_thumbs, image_decode, logo_tile, material_you   -> core, slint
melodia-net       http primitives, 4 fetchers, radio_browser, scrobble providers  -> core, artwork
melodia-platform  tray, media keys, single_instance, logging, crash_report, themes -> core, melodia-ui
melodia-audio     player/                                                          -> core, net
melodia-store     database/, scanner, metadata, watcher, tag_writer, rating_tags   -> core, artwork, audio
melodia-app       library/, tasks/, state/, settings, scrobble, discord, updater   -> all above, melodia-ui
melodia-views     src/ui/                                        -> app, audio, platform, artwork, core, melodia-ui
melodia (bin)     main, boot/, shutdown                                            -> views, app
melodia-testkit   test_support, dev-dependency of every member
```

Three deltas from the issue: audio sits below store rather than beside it, artwork is depended on by
net, store and views rather than being a peer of them, and app and platform carry `melodia-ui`.

`melodia-testkit` works because Cargo permits cyclic **dev**-dependencies between workspace members,
so it can depend back on the crates that dev-depend on it for `DbPool::test_pool()`.

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
      stale doc comment at `:18` in the same commit.
- [ ] **A3. `heap_trim::trim()` moves beside the other platform FFI.** It is a bare
      `libc::malloc_trim` with no task machinery in it, and it is **25 of the 29** `ui` to `tasks`
      edges. `spawn` and `STARTUP_DELAY` stay in `tasks/`, where the one-shot schedule belongs. Not in
      the issue; it is the single cheapest structural win in the list.
- [ ] **A4. The three scan DTOs move to `entities/`.** `ExistingTrackSummary`
      (`database/queries/scan/lookups.rs:70`), `ScannedFile` and `ExtractedMetadata`, applying the
      rule the root `CLAUDE.md` already states. Fixes 7 sites in `database/queries/` plus
      `media/scanner.rs:9` and `:101`.
- [ ] **A5. Persistence and the toast bridge leave `player/`.** Three moves, one commit each:
      - Install `play_count_flusher` in the test contexts that need it, then delete the direct-UPDATE
        fallback at `actions.rs:107-127`. `db: &DbPool` then drops off `execute_actions` and
        `emit_and_execute`.
      - Collapse `toast` and `play_count_flusher` onto one `OnceLock<UnboundedSender<E>>` bridge
        primitive. Producer half goes to core, consumer halves stay where the I/O is.
      - The 30 s periodic save (`handlers.rs:443-460`, `PlaybackMonitorContext.db` and `paths`)
        publishes a snapshot the app layer writes, so the monitor stops owning `DbPool` and
        `write_json_atomic_sync`.
- [ ] **A6. `describe` and the atomic writers move to core.** `services/mod.rs:351`, plus
      `write_json_atomic_sync`, `write_text_atomic_sync` and `load_json_or_default{,_sync}`. Resolves
      4 of the 12 `player` to `services` edges by relocation. The 5 HTTP ones resolve in Phase C by
      the graph, audio depending on net.
- [ ] **A7. `nav_history` and `ui_handles` come off `AppState`.** `state/mod.rs:151,155,277,278,280`
      into a struct the binary owns and passes down. 6 sites in `boot/ui_setup/views.rs`, 18 inside
      `src/ui/` itself. `ui/my_library/tests/my_library_tests.rs:17` does
      `include_str!("../../nav_history.rs")` with source-text assertions at `:1010` and `:1024`, so
      it needs re-pathing. This is the one that actually stops `melodia-views` existing, and the only
      one with structural work in it.

## Phase B: reshape in place, still one crate

- [ ] **B1. Split monolithic `services/mod.rs`.** It is simultaneously the core-primitives module
      (`load_json_or_default*`, `write_*_atomic_sync`, `current_exe`, `is_dev_build`, `redact_home`,
      `describe`) and the HTTP module (`build_http_client`, `http_url`, `is_http_url`, `is_http`,
      `get_capped`, `get_capped_text`, `read_capped`). **Hard prerequisite for everything else**: all
      four media fetchers plus `radio_browser`, both scrobble providers and `updater/github` depend on
      the net half, so nothing can move before this does.
- [ ] **B2. `services/` regroups into net, platform and integrations.** `updater/`'s 24 files straddle
      all three and do not move wholesale: net is `check`, `github`, `manifest`, `install/download`;
      platform is `target`, `linux_pkg`, `system_install`, `install/{staging,verify,swap}`; core is
      `minisign`, `version`, `probe`; the rest is orchestration.
- [ ] **B3. `media/` regroups three ways.** Image tier, ingest, fetchers. Assign `rating_tags.rs` to
      ingest, and pull `services/material_you.rs` into the image tier.
- [ ] **B4. `single_instance.rs:31`'s `crate::media::is_audio_extension`** is the one import between
      that file and a dependency-free platform module. Take the predicate to core.
- [ ] **B5. Move the cross-tier size assertions out of the image tier.**
      `media/artwork/tests/artwork_tests.rs:119-121` reach `ui::grid_prewarm` and `ui::util` to check
      `STORE_MAX_DIM` against the UI's cover tiers. They cannot compile inside a leaf crate, so they
      become integration tests under `tests/`.

## Phase C: extract the crates

Order, each landing green: **core, artwork, net, platform, audio, store, app, views, bin, testkit.**

- [ ] Per-member manifest, modelled on `melodia-ui/Cargo.toml`, which already gets this right:

      ```toml
      [package]
      name = "melodia-<x>"
      version.workspace = true       # never "0.0.0"; see finding 1
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
      bake, `load_dotenv()` moving to the crate holding the `option_env!` sites, and the Windows
      `winresource` embed staying with the binary.
- [ ] `test_support` becomes `melodia-testkit`, a dev-dependency of every member.

**Landing tactic for the 2,721 `crate::` paths.** Each consuming crate re-exports what it took
(`pub use melodia_core::{error, config, entities, utils};` in its `lib.rs`), so `crate::error::AppError`
keeps resolving and the diff stays about topology rather than import churn. Enforcement is unaffected:
a re-export cannot reach a crate absent from the manifest. De-facade in Phase D once the graph is
proven, one crate at a time.

## Phase D: make the repo workspace-native

- [ ] Virtual root manifest, `members = ["crates/*"]`, `exclude = ["winit"]`. Profiles and
      `[patch.crates-io]` stay at the root, as do the four version scrapes.
- [ ] `[package.metadata.deb]` moves to the binary crate with all eight asset paths and `license-file`
      rewritten. Re-key `LICENSE_SHIPPERS`' `("Cargo.toml", ...)` entry in
      `services/tests/mod_tests.rs:398`, which fails loudly and correctly when it moves.
- [ ] `release-build.yml:83` and `:124` take `-p melodia`, or the root declares `default-members`.
      A bare `cargo build` at a virtual root builds every member.
- [ ] The 241 relative `include_str!` hops and the 21 tree-root test anchors. `SRC_DIR` becomes one
      workspace-root definition so the corpus walks keep their full reach rather than narrowing to
      one crate each.
- [ ] All ~134 `.claude/rules` globs, **and the pin that guards them**: drop the
      `glob.contains('*') { continue; }` skip at `test_support_tests.rs:89` so a rotted wildcard fails
      loudly. Without that, this class of breakage recurs the next time anything moves.
- [ ] `CLAUDE.md`'s module map and its "every path below is `src/`-relative" convention, the README
      architecture section, `src/player/CLAUDE.md`'s heading, and the 112 bracketed intra-doc links.
- [ ] Drop the scope-clippy-to-one-crate convention. `feature-unification = "workspace"` is
      nightly-only and the toolchain is pinned to stable, so a scoped invocation reselects features
      for shared dependencies and rebuilds them. Workspace-wide is the only correct command, which a
      workspace makes the natural one anyway.

## Verification

Per commit, unchanged by the split since all three are already workspace-wide:

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Phase boundaries:

- After **A**: `grep -rn 'crate::database\|crate::tasks' src/player/` returns nothing, and
  `grep -rn 'crate::services' src/player/` returns only the HTTP primitives. `grep -rn 'crate::ui' src/tasks/`
  returns nothing.
- After **C**: delete a `path` dependency from one manifest and confirm rustc names the crate in the
  error. The `melodia-views` manifest must not list `melodia-store`; that is the flagship rule
  turning into a compile error, and it is the whole point of the exercise.
- After **D**: `cargo deb -p melodia` plus `scripts/build-{rpm,appimage,tarball}.sh` each produce an
  artifact; `cargo build --timings` against the prerequisite baseline; `/usr/bin/time -v
  target/release/Melodia` for peak RSS. No RSS change is expected, `lto = "fat"` with
  `codegen-units = 1` recovering cross-crate inlining.

One thing gets better rather than staying level: `[profile.dev.package.melodia-audio] opt-level = 2`
becomes possible, so the DSP chain stops being debugged unoptimized.

## Notes

**239 `pub(crate)` sites** widen to `pub` where they cross a boundary, out of 2,056 public items.
The issue counted 209; the tree has grown since. That is the price the literature names, and it is
the same thing as the payoff: it forces the interface to be stated.

**Prior art is argued in the issue** and not restated here. The four sibling checkouts kept beside
this repo (rox, termusic, Symphonia, sonora) are what its crate counts can be checked against.

**`cargo-crate-split` is on crates.io at 0.2.0.** It computes strongly connected components and emits
a minimum cut set, suggest-only, never rewriting source, which suits the no-autofix rule. Its blind
spot is glob re-exports and inference-hidden coupling, so treat its list as a floor cross-checked
against the five cycles above rather than as the answer.

**What this trades away, plainly:** a single `src/` tree any grep reaches in one pass, a test corpus
that has been genuinely good at catching drift, and a packaging path that works today. The bet is
that a compiler-enforced DAG is worth more over the next two years of podcasts and streaming than
those three are, and that the cost is paid once.
