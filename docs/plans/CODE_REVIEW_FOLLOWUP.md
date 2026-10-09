# Code Review Follow-up

An outside review on 2026-09-28 scored the tree 7/10. Checked against the code on 2026-10-04, its
facts held up, its weighting was off in places (it scored several ADR-backed trade-offs as
defects), and it missed three layering problems. This plan holds what is worth doing about it, one
phase per change.

**Order:** phases 0 to 5, 1.5 included, are independent of each other. Phase 6 goes before 7,
because it shrinks four of the monolithic functions 7 would otherwise split by hand. Phase 9 goes
last of the planned phases. Phase 10 is optional; if it is kept, it runs before 9, which would
otherwise trim comments on code 10 deletes.

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
- **About 260 log calls print an error without `describe`.** On the four struct variants that
  drops the reason outright.
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

- [x] `README.md`'s build section and CLAUDE.md's Prerequisites line drop macOS.
- [x] CLAUDE.md's globals count is 48 in 31.
- [x] The albums, artists and genres `close_detail` persist logs go through `error::describe`, as
      playlists' does. Phase 6 folds them into one.

## Phase 1: keep error causes typed

Where: every crate. The variants and constructors are `melodia-core`'s `error.rs`.

- [x] 49 sites moved. ADR 37 keeps `Settings`/`Window`/`Player` as `String` variants, so a cause
      with no struct variant goes on `Io` through the new `AppError::io(msg, source)`, which keeps
      the operation beside the typed cause and prints both. The two settings writers already held
      an `AppError` and now return it unchanged; the tag write's join goes through `metadata`.
- [x] 15 sites keep their text, because a caller dispatches on their variant:
      `FailureKind::classify` (the updater's 12 in `version.rs`, `github.rs`, `install/`), the
      radio form's `form_error` (`stream_decode.rs:90`, and `file_decode.rs:200` beside it, both
      reading `OpenError` as a sentence tail), and the add-folder dialog (`folders.rs:37`).
- Found on the way: none of the wrapped types hides its reason behind `.source()`, so these sites
  lost little text. The bigger gap for a bug report is the reader side, which is Phase 1.5.

## Phase 1.5: every logged error goes through `describe`

Phase 1's reader side. A `Metadata`, `Network`, `Scanner` or `Watcher` error displays its
operation and not its reason (ADR 37), so a log line that prints one with a bare `{e}` reports a
full disk and a permissions failure in the same words. For any other error a bare `{e}` drops
only the chain below it.

- [x] Re-measured on 2026-10-08 and moved: 268 sites in 122 files. 259 printed `{e}`/`{err}`/
      `{error}` inline (12 multi-line, 3 as `{e:?}`), 8 positionally, plus `{persist_err}` and
      `{restore_err}`. 443 calls now go through `describe`, up from 174, and none prints an error
      bare.
- [x] `AppState::persist_blocking` went first, covering its 41 `settings.json` callers.
- [x] One spelling everywhere, `{}` plus `describe(&e)`. Where every site in a file is
      platform-gated, the call spells the full path so the import isn't unused on the other
      platform, and the `hydrate.rs` macro body does the same.
- [x] `log_deferral` and the scrobble `network_failed`/`save_failed` took `Display` and take
      `std::error::Error` now. The one non-error, `ratings.rs`'s `err`, was a string `describe`
      had already built, and is named `reason`.
- [x] All three `{e:?}` moved: Slint's translation error and notify's say in `Display` what
      `Debug` did.
- [x] Not in scope: user-facing text (`paint_error`, a toast's detail) built from an error. What
      the user reads there is a UI decision, not a logging one.
- [x] `crates/melodia/tests/logged_errors.rs` holds it: no `log::` call prints `e`, `err`,
      `error` or a `_err`/`_error` name bare, inline, positional or named. A call it cannot read
      fails the walk, and a floor catches a reader that finds nothing. The reader's own cases are
      table-driven beside it, and `describe` gained chain-depth cases in `error_tests.rs`.
- [x] CLAUDE.md's bullet widened to any error, and names the walk.
- [x] The Windows-only sites (`tray_icon_backend.rs`, `souvlaki_backend.rs`, `parked_loop.rs`,
      `main.rs`'s Windows arms) pass fmt, clippy and the tests on Windows (2026-10-08). No
      `describe` call sits behind a macOS-only gate.

## Phase 2: take Slint out of the audio stack

The chain was `melodia-audio` → `melodia-net` → `melodia-artwork` → `slint`. Net named
artwork only in `media/fetch/station_logo.rs` and `deezer.rs`.

- [x] Both files split at the store boundary. `station_logo::fetch` returns a `FetchedLogo` (bytes
      plus the extension its content type names), and `deezer::download_artist_image` returns the
      bytes. `logo_discovery.rs` still takes the scheme guard and the timeout from `station_logo`.
- [x] The station logo's store half went *down*, into artwork's `media/image/logo_tile.rs` as
      `logo_tile::store` with the `MIN_LOGO_DIM` floor, not up into `library/radio/logos.rs`.
      `logo_tile` existed only for this caller, and everything the half touches was already
      artwork's, so app would have held pixel code for no reason. `library::radio::fetch_logo` now
      fetches, then stores on the blocking pool. Deezer's one store line went up into
      `services/artist_images.rs` as planned.
- [x] Discord only needs the lookup (`deezer::search_album_cover`, a URL), so nothing moved to
      integrations.
- [x] Net's manifest drops `melodia-artwork`, `image` and the dev-dep `tempfile`. Exit check:
      `cargo tree -i slint` finds nothing for audio, playback, engine, net and integrations.
      **Store still finds it**, and the original check was wrong to include it: store names
      `melodia-artwork` itself for ingest (`scanner.rs`, `metadata.rs`, `cover_embed.rs`), and
      ADR 33 keeps artwork on Slint.
- [x] No new crate.
- [x] The logo store tests moved to `media/image/tests/logo_tile_tests.rs` beside the floor, the
      `.slint` pin with them. The two that went through a socket only to reach the store call it
      directly now; net keeps the HTTP cases, plus one checking the bytes come back as served.
- [x] CLAUDE.md's `media/` bullet, `.claude/rules/radio.md` (paths and the pin's name), and the
      two `.slint` comments that named `media::fetch::station_logo` now point at `logo_tile`. The
      `artwork-image.slint` one also dropped "plain-http", stale since cleartext logos were
      admitted.

## Phase 3: drop `melodia-ui` from `melodia-app`

`tasks/updater_daily.rs` held a `Weak<AppWindow>` and wrote the `MelodiaUpdater` globals through
three painters that copied views' `callbacks/updater/paint.rs` line for line, and
`tasks/rss_sampler.rs` called `slint::spawn_local`.

- [x] `updater_daily::spawn` takes a painter, `impl Fn(PanelPaint)`, where it took the window.
      `ui::callbacks::check_painter` builds it over views' existing painters, so the copies
      are gone. This replaced the planned `watch` on `AppState`: no channel, no subscriber and no
      `AppState` field, and every write lands exactly as it did before.
- [x] `rss_sampler::sampler` hands its loop back, and `boot::ui_setup::install_rss_sampler` spawns
      it on the UI thread with the view tag `main.rs` used to build.
- [x] App's manifest drops `melodia-ui`, `slint` and `async-compat`. A `.slint` edit no longer
      rebuilds app.
- [x] CLAUDE.md's `tasks/` bullet and its Memory Discipline line now describe it.
- [x] Found on the way: the Check button and the daily task each had their own copy of what
      follows a check, and the copies had drifted. Both now run `services::updater::run_check`.
      It also fixes the ETag cache: an "available" response's tag was cached, so the next daily
      check got a `304` and showed nothing, and a dismissed update never came back. A tag is now
      sent only while the manifest it names offered this build nothing newer.

## Phase 4: SQL lives in `melodia-store`

Re-measured on 2026-10-09: app ran 3 raw SQL strings (`tasks/tag_backfill.rs`,
`tasks/queue_prune.rs`, `file_event_processor/reconcile.rs`) and opened 7 write transactions, not
6: `library/import.rs`, `library/radio_files.rs`, `library/tags.rs`, three in `library/scan/` and
`reconcile.rs`. Six of its functions took a `&mut Transaction`, and one type derived `FromRow`
(`queue_prune.rs`).

- [x] The three queries are `queries::track::existing_ids`, `mark_every_track_stale` and, for the
      watcher's move candidates, `lowest_id_by_hash_on`. That last one was already store's private
      `ingest::batch_lookup_by_hash` word for word, and ingest and the watcher share it now.
- [x] Each transaction is one store function that opens, runs and commits it:
      `artwork::forget_paths` (the repair, roll-ups included), `ingest::commit_import`,
      `ingest::commit_scan_chunk`, `scan::commit_scan` (the scan's last write, taking `Ingested`
      and `orphans_of` with it), `radio::import_stations`, `scan::commit_retag` (the tag edit's
      commit) and `scan::apply_watch_batch`. Tag reads, the rename's fallback `stat`, the
      blocklist check and override validation stay in app and run before the call.
      `apply_watch_batch` keeps both halves of moved-file detection in one function: candidates
      resolved before the transaction opens, deletes applied last.
- [x] The column lists are `queries/track/columns.rs`: one `Columns` type and eight statics, where
      core had seven identical `OnceLock` joins and two identical alias caches. `entities/track.rs`
      went from 720 lines to 473.
- [x] `sqlx` is a dev-dependency of app, as it already was of the binary: the suites seed rows and
      read them back, and production code can't name it. 34 store functions that take a
      transaction, a connection or an executor went crate-private (radio's four into `radio.rs`
      alone), with `NameCache`, `FolderResolution` and `chunked_in_query`, so app can still open a
      transaction but has nothing to hand it to. `ResolvedIds` stayed `pub`: it carries no
      transaction, and narrowed it trips `struct_field_names`.
- [x] The tests followed their code: the watch handlers' twelve to
      `queries/tests/watch_batch_tests.rs`, `orphans_of`'s and `Ingested`'s to store, the stale
      pass's three to `track_tests.rs`. The rename mtime pin split across the seam: store writes the
      mtime it is handed, and app's conversion hands it `meta`'s or a `stat`'s.
- Found on the way: `scan::update_track_artwork_if_missing` had no caller outside its two tests,
  which narrowing made visible; both are gone. `ingest_scanned_files` still checks its moved-file
  candidates on disk while its write transaction is open. Left as it is: moving that check ahead of
  the transaction changes how a concurrent delete races it, which is a change of its own.

## Phase 5: library functions take what they read

Re-measured on 2026-10-09: 211 pub library functions took `&AppState`, and 141 of them read one
field and nothing else, 78 `state.paths` and 63 `state.db`. 67 of the `paths` ones were in
`library/settings/`.

- [x] Module by module, `settings/` and `window.rs` first, then the `db` readers. Five more
      narrowed once their callees had: `entity_tracks::track_ids_for`, `clipboard::entity_lines`,
      `smart_playlists::recount` and `radio::logos::{for_urls, adopted}`. 65 pub functions still
      take `&AppState`; each reads several fields, bar the exceptions below.
- [x] Signatures take `&Paths` or `&DbPool` and callers pass `&state.paths` or `&state.db`. No new
      context types. `AppState::persist_blocking` hands its closure `&Paths` and moves one `Arc`
      into the blocking task, where it cloned the whole state; views' six
      `persist: fn(&AppState, bool)` helpers take `fn(&Paths, bool)`.
- [x] 26 private twins are gone (17 over `paths`, 9 over `db`). Each existed so a test could reach
      a body its `&AppState` door hid, and the tests call the public function now. A twin stays
      where its door reads several fields (`window::set_always_on_top`, `playlists`, `queue`) or
      fills in a seam (`write_archive`'s clock, `write_use_native_titlebar`'s desktop probe).
- [x] Left on `&AppState` deliberately: the `SharedFlag` readers, since several flags share the
      type and a narrowed signature would take the wrong one, and the search-history and
      `scan::cancel` forwards, which read neither field.
- [x] `state/contexts.rs` and `library/mod.rs` say what the rule is now. The latter had argued the
      opposite, keeping the door on `&AppState` so views never held a database handle: views
      passes `&state.db` now, and still cannot name the type or reach a query.
- Found on the way: `lyrics::{resident_text, forget_all}` read `state.runtime` across a line break,
  which the first count missed, so `library::lyrics` keeps `&AppState`. Three intra-doc links
  were already broken (`radio/authoring.rs`, `lyrics/store.rs`, views' `radio/facets.rs`) and are
  fixed.

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
- [ ] The rest (`scan_one`, `ingest_scanned_files`, `spawn_playback_monitor`, `populate`) split
      by stage.
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
- Anything CLAUDE.md, a rule or an ADR points at ("argued at `apply_watch_batch`"). Condense it and
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

## Phase 10 (optional): make dead code visible to the lints

**Optional, and only after a discussion.** That discussion may drop the phase, or widen it where
another solution can be adapted.

Phase 4 found `scan::update_track_artwork_if_missing` with no caller outside its own tests, and
nothing had flagged it. `dead_code` judges one crate at a time and treats any `pub` item reachable
from the crate root as API another crate might use, so an unused `pub fn` in a library crate is
invisible to it. Narrowing the function to `pub(crate)` is what exposed it: under `--all-targets`
the library is also built without its tests, and that build found no caller.

- [ ] Narrow `pub` to what another crate imports, crate by crate. A private or `pub(crate)` item
      is already covered by `dead_code`, test-only callers included. `unreachable_pub` in
      `[workspace.lints.rust]` catches the `pub` items that aren't exported anyway: measure what it
      raises first and fix each by hand. It can't see items exported through a `pub mod` tree, so
      those modules narrow too, keeping `pub` only on what another crate names. Not alongside
      `clippy::redundant_pub_crate`, which pushes the other way.
- [ ] A corpus walk in `crates/melodia/tests/` for the `pub` items no other crate uses, a question
      no lint can answer since each crate compiles alone. It collects each library crate's `pub`
      free functions and types and asserts each appears in another crate's source, owing the five
      things CLAUDE.md asks of a walk, with the exemptions (the `#[doc(hidden)]` fixtures among
      them) held to an exact count. A text walk, so trait methods, generated code and common names
      like `new` stay out of it.
- [ ] Whatever either step finds dead is deleted, not suppressed.
- Ruled out: `dead_code_pub_in_binary` covers binary crates only, and CLAUDE.md keeps it off under
  `--all-targets`.
- Candidates for widening it: a tool that answers the cross-crate question better than a text
  walk, and unused dependencies (`cargo-machete` or `cargo-shear`, both on stable;
  `unused_crate_dependencies` is too noisy under `--all-targets`, every integration-test crate
  seeing every dev-dependency).

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
