# Code Review Follow-up

An outside review on 2026-09-28 scored the tree 7/10. Checked against the code on 2026-10-04, its
facts held up, its weighting was off in places (it scored several ADR-backed trade-offs as
defects), and it missed three layering problems. This plan holds what is worth doing about it, one
phase per change.

**Order:** phases 0 to 5 are independent of each other. Phase 6 goes before 7, because it shrinks
four of the monolithic functions 7 would otherwise split by hand. Phase 9 goes last.

## What we see

- **A fix to a detail view lands four times.** Albums, artists, genres and playlists wire the same
  callbacks in near-copies (roughly 60 to 90% alike), and the copies have started to drift: three
  `close_detail` logs print a bare `{e}` where playlists goes through `describe`.
- **Audio, playback, engine and store all build Slint, winit and femtovg.** `melodia-net` reaches
  `melodia-artwork` for two fetchers that store what they download, and the audio crate needs net
  for its HTTP helpers.
- **A `.slint` edit recompiles the whole command layer.** `melodia-app` still names `melodia-ui`
  for two tasks, the edge ADR 16 rules out.
- **About 50 errors lose their cause on the way up.** A typed error gets formatted into a `String`
  variant, so `describe` has nothing to walk, and a bug report names the operation without the
  reason.
- **Most library functions take the whole `AppState` and read one field of it**, so testing one
  means building all of it.
- **Some wiring functions are monolithic.** The largest is about 390 lines of eleven closures, and
  the lints that would flag it are off.
- **Production code carries about one comment line for every two lines of code.** Comparable Rust
  audio projects sit between about 0.1 and 0.3.

## Where we stand (rough, 2026-10-04)

The figures are rounded on purpose: the tree will move before this plan is worked, so each phase
re-measures its own numbers first.

- Production Rust: ~69k code lines, ~31k comment lines (a ratio of ~0.45). The Slint tree: ~22k
  code, ~8.8k comment (~0.40). Tests already sit at ~0.24.
- No production file is over 800 lines, but the largest are at ~780 (`stream_source.rs`, the
  engine's `backend/mod.rs`). Any phase that grows a file checks the cap.
- About 4,300 production functions, with a median of 7 lines. About 110 are over 50 lines. Clippy's
  `too_many_lines` flags 35 at its default of 100, and `cognitive_complexity` flags 3 at the
  configured 30.

## Every phase

- [ ] Re-measure what the phase targets before touching code, and rescope if it moved.
- [ ] Gate: `cargo fmt --all --check`, `cargo clippy --all-targets --locked --workspace -- -D warnings`,
      `cargo test --locked --workspace`. Existing tests the change breaks are updated in the same
      phase.
- [ ] Update CLAUDE.md, the `.claude/rules/` entry or the module doc that names what moved, in the
      same phase.
- [ ] No production Rust file past 800 lines.

## Phase 0: small corrections

- [ ] `README.md`'s build section (~line 243) says "macOS and Windows need nothing extra", while the
      badge and the release matrix are Linux and Windows only. Drop macOS there or mark it
      untested, and do the same to CLAUDE.md's Prerequisites line.
- [ ] CLAUDE.md's "the 47 globals in 30 feature-scoped files" is stale (it was 48 in 31 at the
      check). Recount.
- [ ] The albums, artists and genres `close_detail` persist logs go through `error::describe`, as
      playlists' does. Phase 6 folds them into one, but the log is wrong today.

## Phase 1: keep error causes typed

Where: every crate. The variants and constructors are `melodia-core`'s `error.rs`.

- [ ] Find the sites: a typed error turned into `AppError::{Settings,Window,Player,Queue,…}` through
      `format!("…{e}")` or `e.to_string()`. There were about 50 at the check, including
      `main.rs:141` and the `JoinError`s in `queue.rs`, `window.rs` and `playback.rs`.
- [ ] Each one moves to a spelling that keeps the source: plain `?` where a `#[from]` exists (`Io`,
      `Database`, `Migration`), the struct-variant constructors (`metadata`, `network`, `watcher`,
      `scanner`), or `io_source(e)`. `main.rs:141` becomes a plain `?`, since `Builder::build()`
      returns an `io::Error`. A `JoinError` goes through `io_source(e)`.
- [ ] A message with no cause behind it stays a `String` variant. That is allowed, and it is not
      what this phase is about.

## Phase 2: take Slint out of the audio stack

Today the chain is `melodia-audio` → `melodia-net` → `melodia-artwork` → `slint`. Net names
artwork only in `media/fetch/station_logo.rs` and `deezer.rs`.

- [ ] Split each of those two files at the store boundary. The HTTP half (fetch, cap, check the
      bytes) stays in net and returns bytes. The half that writes into the artwork store moves up
      to its callers, which already sit above artwork: `library/radio/logos.rs` and
      `services/artist_images.rs` in app, and `discord/artwork.rs` in integrations. Net's own
      `logo_discovery.rs` keeps calling the fetch half.
- [ ] Read the Discord call site first. If it only needs the lookup (a URL), the store half goes
      to app alone. If app and integrations both need it, it goes to integrations, which app
      already sits above.
- [ ] Drop `melodia-artwork` from net's manifest. Exit check: `cargo tree -p melodia-audio -i slint`
      finds nothing, and the same holds for playback, engine and store.
- [ ] No new crate. CLAUDE.md records how the first new member broke the release workflows by name.
- [ ] Update CLAUDE.md's `media/` bullet, which describes `fetch/station_logo.rs` as the logo
      ingest that enforces the store's source-size floor, and `.claude/rules/radio.md` if it names
      the file.

## Phase 3: drop `melodia-ui` from `melodia-app`

Today `tasks/updater_daily.rs` holds a `Weak<AppWindow>` and writes the `MelodiaUpdater` globals,
and `tasks/rss_sampler.rs` calls `slint::spawn_local`.

- [ ] `updater_daily` publishes its result on a `watch` on `AppState`. A subscriber in
      `melodia-views` writes the globals, through `ui::signal::on_signal` or the bridge pattern.
- [ ] `rss_sampler` gets its UI-thread spawn from its caller, the way it already gets the view-tag
      closure from `main.rs`.
- [ ] Remove `melodia-ui` from app's manifest, and `slint` too if nothing else in app uses it.
- [ ] Update CLAUDE.md's `tasks/` bullet, which documents this edge as the one exception.

## Phase 4: SQL lives in `melodia-store`

Today app runs 3 raw SQL strings (`tasks/tag_backfill.rs`, `tasks/queue_prune.rs`,
`file_event_processor/reconcile.rs`) and opens about 6 transactions (`tags.rs`, `import.rs`,
two in `library/scan/`, `reconcile.rs`, `radio_files.rs`). About 6 of its
functions take a `&mut Transaction`, and one type derives `FromRow` (`queue_prune.rs`).

- [ ] The three queries move into `queries::*` as named functions.
- [ ] Each transaction becomes a store function that opens, runs and commits it, taking plain data.
      Non-SQL work (file I/O, tag reads) happens before or after it, never inside a store function
      that holds the write connection.
- [ ] The SQL column lists in core's `entities/track.rs` are used only by store, so they move to
      store. That also takes a large bite out of a ~720-line file.
- [ ] Remove `sqlx` from app's manifest.

## Phase 5: library functions take what they read

Today about 200 of the library's ~270 pub functions take `&AppState`. About 145 of those read only
`state.paths` (~80) or only `state.db` (~65). In `library/settings/` it is about 70 of 80, nearly
all `paths`-only.

- [ ] Work module by module, starting with `library/settings/` (`settings/playback.rs` alone has 9
      `paths`-only functions). Go bottom-up: a function that passes `state` onward narrows after
      its callees do.
- [ ] The signature becomes `&Paths` or `&DbPool`, and callers pass `&state.paths` or `&state.db`.
      `AppState` stays the composition root. Functions that really use several fields keep taking
      it. No new context types.
- [ ] Rewrite `state/contexts.rs`'s module doc (~lines 6 to 10), which claims library modules touch
      most of `AppState`.

## Phase 6: one wiring for the four detail views

Today albums, artists, genres and playlists each have a `detail.rs` and a `callbacks/detail.rs`,
about 2,600 lines together and 60 to 90% alike once the view names are normalized.
`RowSelectionView` (`list_selection.rs`) and `TrackListColumnState` (`track_list_view.rs`) already
show that a trait over generated globals works, yet four comments say it can't.

- [ ] Re-read the four wirings first. Recent work (copy entries, flyout menus) may have added
      callbacks.
- [ ] Add a `DetailGlobal` trait over the four detail globals, implemented by one macro of the same
      shape as `RowSelectionView`'s.
- [ ] Add a generic `wire_track_detail::<G>` that captures `G::as_weak()` (a `'static` handle in
      Slint 1.16) rather than borrowing the UI. It wires the shared set: play-row, favorite,
      rating, the three selection callbacks, columns, shuffle, play-next, queue, filter and the
      sorts.
- [ ] What differs stays per view: close-detail, playlists' reorder, remove, edit-artwork and
      natural sort, artists' album strip, and each view's `open_*_with`, `refresh_detail` and
      `fetch_*`.
- [ ] The four `*DetailState` structs share a `DetailCache`.
- [ ] The shared wiring answers to no single view, so it goes under `ui/callbacks/`. The new home
      goes into `CALLBACK_HOMES` in the same change, since that walk checks for equality.
- [ ] `search/selection.rs` drops its own copy of `handle_curated_click` and uses `RowSelectionView`.
- [ ] Fix the four stale comments: `detail_view.rs:3-7`, `cross_tab_nav.rs:100-103`,
      `my_library/filter.rs:31-33` and `macros.rs:159-161`.
- [ ] Leave the five list views alone. Their overlap buys little.
- Expected: roughly 350 to 400 lines gone (~15% of those files), and one place to fix instead of
  four.

## Phase 7: break up the monolithic functions, then turn the lints on

The largest today, roughly: playlists `callbacks/dialog.rs::wire` (390), `main` (305), playlists
`callbacks/detail.rs::wire` (270, shrinking in phase 6), `winit_filter::install` (210), the other
three detail `wire`s (150 to 190, phase 6), `callbacks/tags/open.rs::populate` (175),
`search/callbacks/results.rs::wire` (165), `card_actions::wire` (160), `ingest_scanned_files`
(140), `spawn_playback_monitor` (140) and `library::scan::scan_one` (110).

- [ ] Wiring functions get one named function per callback, and `wire` becomes the list of calls.
- [ ] `main()` splits into named boot steps, kept in order. `crates/melodia/src/tests/main_order_tests.rs`
      pins that order by reading `main.rs` as text, so its pins move with the code in the same
      step.
- [ ] The rest (`scan_one`, `reconcile`, `ingest_scanned_files`,
      `spawn_playback_monitor`, `populate`) split by stage.
- [ ] Turn on `cognitive_complexity` in `[workspace.lints.clippy]`. Its threshold of 30 is already
      set in `clippy.toml`, where it does nothing until the lint is on.
- [ ] Remove `too_many_lines = "allow"` from `Cargo.toml` once the count is down. If the leftovers
      still need it, set `too-many-lines-threshold` in `clippy.toml` to what the tree meets and
      lower it over time. A function that must stay long gets `#[expect(…, reason = "…")]`, never
      `allow`.

## Phase 8: `Dialog.kind` as an enum

`globals/dialog.slint`'s `kind` is a `string` with about 23 values, matched by the dispatcher's
arms. A typo builds fine and gives a button that does nothing.

- [ ] Add a Slint `enum` for the kinds and type `kind` with it. The dispatcher arms and every Rust
      setter use the generated enum.
- [ ] If the toast kinds the `Notifications.action` dispatcher routes on are still strings, they
      get the same treatment. `signal_path.rs`'s `REFUSAL_TOAST_KIND` documents this same failure
      mode: a mismatch paints a Details button that does nothing.

## Phase 9: comments down to about 0.25

**Target:** roughly 0.25 comment lines per code line, for production Rust and for the Slint tree.
Today they are ~0.45 and ~0.40. At today's size that means cutting about 14k Rust comment lines and
about 3k Slint ones.

The ratio is comment lines divided by code lines, as measured by:

```bash
# production Rust (tests and the testkit excluded)
cloc --include-lang=Rust --exclude-dir=tests,target,melodia-testkit,benches --not-match-f='_tests\.rs$' crates
# the Slint tree
cloc --force-lang=C++,slint --include-ext=slint crates/melodia-ui/ui
```

This phase runs last because phases 1 to 8 delete and move code. Compressing first would mean
editing comments that are about to vanish, and the code those phases write is held to the target
from the start.

**What goes:**

- The changelog register: the bug story, "used to", "no longer", "previously", what shipped before.
- Step-by-step narration of the mechanism, and anything that restates the code, the name or the
  signature.
- Second copies. An argument made in a doc comment and again in CLAUDE.md, a rule or an ADR keeps
  one copy, and CLAUDE.md's own tiering says which.
- Long module headers. 15 of the 20 longest comment blocks are `//!` headers. Cut each to the
  module's contract plus its one non-obvious why.
- Multi-paragraph rationale where one or two sentences carry it.

**What stays (shortened, never deleted):**

- `// SAFETY:` comments.
- The why behind ordering, lock order, platform quirks and magic constants.
- Couplings across trees, such as an index order a `.slint` list follows (`signal_path.rs`'s
  `fallback_index`).
- Anything CLAUDE.md, a rule or an ADR points at ("argued at `process_batch`"). Condense it and
  keep the pointer true.

**How the work runs:**

- Commits are comment-only, with no code changes mixed in, so each diff reviews as prose.
- Never touch `migrations/*.sql`. sqlx checksums a shipped migration's whole file, comments
  included.
- Some source-text tests read comment text or anchors, so run `cargo test` after each slice.
- Re-read `.claude/rules/code-style.md` before each slice.
- The target is tree-wide. A crate may end a little above 0.25 if another ends below it.

**Slices, worst ratio first,** so the bar gets set on small crates before the large ones. The
figures are rough, from 2026-10-04:

| Slice | Ratio now | Lines to cut |
|---|---|---|
| melodia-artwork | 0.74 | ~600 |
| melodia-net | 0.60 | ~700 |
| melodia-platform | 0.51 | ~500 |
| melodia-audio | 0.48 | ~700 |
| melodia-engine | 0.47 | ~700 |
| melodia-core | 0.46 | ~700 |
| melodia-app | 0.46 | ~2,400 |
| melodia-playback | 0.45 | ~1,200 |
| melodia (binary) | 0.44 | ~200 |
| melodia-views | 0.44 | ~5,400 |
| melodia-store | 0.37 | ~600 |
| melodia-integrations | 0.30 | ~150 |
| Slint tree | 0.40 | ~3,200 |

- [ ] melodia-artwork
- [ ] melodia-net
- [ ] melodia-platform
- [ ] melodia-audio
- [ ] melodia-engine
- [ ] melodia-core
- [ ] melodia-app, split into `library/`, `tasks/`, `services/` and `state/`
- [ ] melodia-playback
- [ ] melodia (binary)
- [ ] melodia-views, split by `ui/` subtree, one slice per few views
- [ ] melodia-store
- [ ] melodia-integrations
- [ ] Slint tree, split into `components/`, `views/`, `globals/` and `layout/` plus the root files
- [ ] Re-measure both ratios and write the target into `.claude/rules/code-style.md`, so new code
      holds it.

## Not in scope

- **Removing `FromRow` and `From<sqlx::Error>` from core.** The orphan rule would force 20 mirror
  row types, or a `map_err` at every `?` in store. The column lists still move, in phase 4.
- **`query!` macros.** They need a live database or a committed query cache in CI, and every query
  already runs against real SQLite in the tests.
- **`SELECT *` cleanup.** 8 of those queries are test-only, and most of the rest read the `*_stats`
  views.
- **Documented trade-offs:** `process::exit(0)`, `EqSource` (ADR 10), one `AppError` (ADR 37),
  artwork depending on Slint (ADR 33) and `source_allows` (ADR 8).
- **Flattening long module paths.** Churn without payoff.
- **Purging statics.** Only about 29 of ~84 are truly process-wide mutable state.
- **macOS CI.** macOS isn't shipped, so phase 0 corrects the docs instead.
- **Bulk-deleting the UI source-text tests.** Prune one when it blocks a refactor.
- **Discarded `spawn_local` results.** They can only fail when no event loop exists, which can't
  happen once the window does.
