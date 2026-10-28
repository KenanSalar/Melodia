# Plan: Compress the subsystem rules, and take their citations out of the code

Status: **in progress**, Phase 10 next · Owner: Kenan · Created: 2026-10-27 · branch `docs/trim-comments`

> **Measured 2026-10-27** against `a3963bb3` plus Phase 1's changes. Every figure below comes from
> a command run that day. Each phase measures its rule again before it starts (Every rule, step 1),
> since the branch moves under it.

Working doc. Delete when every phase has landed (Phase 10).

## Context

**What you see today.**
- **The subsystem rules are paragraph-sized bullets**, most of them restating a module `//!` or a
  doc comment. Root `CLAUDE.md` calls that restatement the copy to delete. The seven left run from
  7 KB (`symphonia.md`) to 25 KB (`ci-packaging.md`), and Progress has each one's size.
- **Code comments still cite rules**, which root `CLAUDE.md` bans ("A code comment never cites a
  `.claude/rules/` file"): 34 citations across `crates/` and `.github/`, listed under each phase.
- **Some entries are stale**: they name code that no longer exists, or a mechanism a dependency
  has moved since. ui-patterns' pass found 3 and slint-pitfalls' 7. audio-stack's found 4, one of
  them a claim about another subsystem of ours, desktop-shell's 1 and library-data's none.
- **`diagnostics.md` holds nothing its anchors don't**, except one declined decision.
- **`slint.md`'s font advice is wrong for this tree.**

**Done so far on this branch**, and the reference for register and result:

| Rule | Commit | Before | After |
|---|---|---|---|
| `ui-patterns.md` | `5f6559f0` | 1,715 lines / 136,354 bytes | 371 / 27,459 (20%) |
| `slint-pitfalls.md` | `a3963bb3` | 946 / 77,705 | 419 / 28,498 (37%) |
| `audio-stack.md` | `fbee5d01` | 122 / 37,701 | 165 / 11,746 (31%) |
| `desktop-shell.md` | `df93eb10` | 299 / 22,352 | 128 / 8,345 (37%) |
| `library-data.md` | `312554ea` | 314 / 24,572 | 124 / 8,403 (34%) |
| `ci-packaging.md` | `52c215ed` | 323 / 24,758 | 122 / 7,996 (32%) |
| `radio.md` | `3cb8af76` | 181 / 13,308 | 107 / 6,492 (49%) |
| `visualizer.md` | `ac2b1b69` | 159 / 11,280 | 51 / 2,745 (24%) |
| `updater.md` | `62386f54` | 160 / 11,471 | 71 / 4,287 (37%) |
| `symphonia.md` | `09da3fd3` | 112 / 7,296 | 52 / 2,899 (40%) |
| `diagnostics.md` | `6beaeaa0` | 52 / 3,006 | deleted |

Read the first two commits before Phase 2: `git show <sha> --stat`, then the rule as it landed, then
a few of the comment diffs.

**Outcome.**
- **Each rule keeps, per entry,** the contract in one sentence, its why, the rule to follow, and
  the anchor file or pin that holds it. Nothing that restates a module doc.
- **No code comment cites `.claude/rules/`** in `crates/`, `scripts/`, `.github/` or `packaging/`.
  Each one states its own reason instead.
- **`diagnostics.md` is gone**, its one unrecorded decision moved to its anchor.
- **`slint.md`'s font bullet describes what the tree does.**
- **What keeps working:**
  - every rule-to-rule, `CLAUDE.md` and ADR citation still lands on an entry under a recognisable
    lead;
  - every frontmatter is unchanged, so every rule loads on the same files;
  - no code changes.

## Decisions settled here

1. **A phase's code diff is comment-only**, and a mechanical check says so (Verification).
   - Each comment that cited a rule states its own reason in one clause: the rule's mechanism or
     cure as it applies at that line. Where the reason is already there, only the pointer goes.
   - Comment lines only, roughly net zero in length (Phase 1: 14 lines out, 14 in, 186 characters
     more), lines at most 100 columns, no em dashes.
   - A test module doc citing the rule loses the citation too.
   - One Edit per site, never `sed`.
2. **Staleness before compression.** An entry naming code that no longer exists, or a mechanism a
   dependency has moved, is fixed or cut, never compressed.
   - Dependency internals are read at the version `Cargo.lock` pins, under
     `~/.cargo/registry/src/index.crates.io-*/`. The vendored winit fork is `winit/` in the repo.
   - A claim about another subsystem of ours is a claim like any other: Phase 1 found "`Loading`
     clears Discord presence" false (`discord/model.rs` holds the card through `"loading"`).
3. **A comment whose stated reason step 2 makes false is rewritten**, whether or not it cites the
   rule, and listed in the report.
4. **Cut hard, and expect a second pass.** audio-stack's first pass landed at 52% and its second at
   31%. Per entry, cut:
   - history: "shipped as", version stories, what broke once;
   - symptom narration past one sentence;
   - lists of which files do something today;
   - full test-function names. Name the pin's file instead.

   Where an entry restates a doc, print the anchor's doc (Tools) and check it actually carries the
   argument. Then keep only the pointer or the cross-file part. Wrap at 100 columns.
5. **The frontmatter stays byte-identical.** `crates/melodia/tests/rules_globs.rs` parses it, and its
   globs decide where the rule loads.
6. **Citations into a rule stay, and constrain it.** Rule-to-rule, `CLAUDE.md` and ADR citations
   are grepped before writing, and every entry they name keeps a recognisable lead. An accepted ADR
   is never rewritten (`docs/adr/README.md`), so an ADR citing a rule pins that rule's entry.
7. **The gate is `cargo fmt --all --check`, plus clippy whenever a `.rs` file changed**, comment
   lines included (Open questions 1). No tests, the edits being comments and markdown. A comment
   edited under
   `crates/melodia-playback/src/player/playback/output/` also gets bare
   `rustfmt --edition 2024 --check`: cargo fmt skips the modules declared inside `output/mod.rs`'s
   `cfg_select!` (`alsa`, `alsa_volume`, `hardware_volume`, `realtime`, `reserve`,
   `endpoint_volume`, `mmcss`, `wasapi`, `wasapi_clock`, `wasapi_formats`, `wasapi_offered`,
   `unsupported`).
8. **Claude never commits or stages.** Each phase ends in its report, and Kenan runs `/ship`.
9. **One phase per rule, in the order below.** Phase 5 groups three smaller rules, each its own
   item and its own report.

## Structure (who owns what afterwards)

| Where | Owns |
|---|---|
| `///` and `//!` at the anchor | The contract and its one non-obvious why. The first place to look |
| `.claude/rules/<subsystem>.md` | What no single anchor can hold: a coupling between crates or trees (Rust and `.slint`, Rust and CI), a comparison across peers, a prohibition violable from outside the owning tier. Never a restated doc comment. Never cited from code |
| Root `CLAUDE.md` | The rule table, and prohibitions violable from anywhere, each with its "argued at" pointer |
| `docs/adr/` | Why this and not the obvious alternative. Not edited here |
| This file | The remaining work. Deleted in Phase 10 |

## Every rule

1. **Read and find what points in.**
   - Read `.claude/rules/code-style.md`, then the whole rule.
   - Measure it (Verification) and compare with Progress. Rescope if it moved.
   - Grep every reference to the rule in `crates/`, `scripts/`, `.github/` and `packaging/`, plus
     `CLAUDE.md`, `README.md`, `CONTRIBUTING.md`, `docs/` and the other rules. List them by file.
   - Grep `.cargo/` and the root `*.toml` too, and read the files the rule anchors for an unnamed
     pointer ("the rule file"). Phase 4 found three citations the name grep missed.
2. **Staleness check**, two halves.
   - (a) Extract every backticked identifier the rule names that is our code, and grep the tree for
     each (Tools). A word hit doesn't prove `X::y` lives on `X`, so check qualified names against
     their definitions. An entry naming code that no longer exists gets fixed or cut.
   - (b) For each dependency the rule leans on, read the source at the pinned version. Last pass
     this found five slint-pitfalls entries that Slint 1.18.1 and femtovg 0.27 had moved out from
     under, so take no mechanism on trust. Claims about our own other crates count too.
3. **Sweep the code citations**, one Edit per site (Decision 1). Where step 2 makes a stated reason
   false, rewrite it and list it (Decision 3).
4. **Compress the rule in one pass, then a second** (Decision 4). Keep the frontmatter
   byte-identical.
5. **Verify and report**, with numbers from commands (Verification):
   - lines and bytes, before and after;
   - zero citations of the rule left in all four trees;
   - every changed code line a comment line;
   - no changed line over 100 columns, and no em dashes added;
   - the identifier check clean;
   - every entry the other rules, `CLAUDE.md` and the ADRs cite still present;
   - `cargo fmt --all --check` clean, plus `rustfmt` and clippy where Decision 7 asks.

   Then report what was cut as stale, which dependency claims were verified against source and
   which couldn't be, and anything that couldn't be placed. Stop there.
6. **Fill in Progress and tick the phase.** Hand anything found for a later phase to that phase's
   notes.

## Progress

"Code citations" counts the comments citing that rule in `crates/`, `scripts/`, `.github/` and
`packaging/`. Find a phase's commit with `git log --oneline -- .claude/rules/<rule>.md`.

| Phase | Rule | Before | Code citations | After |
|---|---|---|---|---|
| 1 | `audio-stack.md` | 122 lines / 37,701 bytes, 54 over 100 | 8 in 7 files (plus 1 to `slint-pitfalls.md`) | 165 / 11,746 (31%), 0 over 100, 0 citations |
| 2 | `desktop-shell.md` | 299 / 22,352, 4 over 100 | 8 in 6 files | 128 / 8,345 (37%), 0 over 100, 0 citations |
| 3 | `library-data.md` | 314 / 24,572, 9 over 100 | 6 in 6 files | 124 / 8,403 (34%), 0 over 100, 0 citations |
| 4 | `ci-packaging.md` | 323 / 24,758, 9 over 100 | 1 named, plus 3 the grep missed | 122 / 7,996 (32%), 0 over 100, 0 citations |
| 5.1 | `radio.md` | 181 / 13,308, 6 over 100 | 7 in 5 files | 107 / 6,492 (49%), 0 over 100, 0 citations |
| 5.2 | `visualizer.md` | 159 / 11,280, 1 over 100 | 7 in 6 files | 51 / 2,745 (24%), 0 over 100, 0 citations |
| 5.3 | `updater.md` | 160 / 11,471, 5 over 100 | 1 | 71 / 4,287 (37%), 0 over 100, 0 citations |
| 6 | `symphonia.md` | 112 / 7,296, 5 over 100 | 0 | 52 / 2,899 (40%), 0 over 100, 0 citations |
| 7 | `diagnostics.md` (deleted) | 52 / 3,006 | 0 | deleted; `rules_globs.rs` sees 176 paths |
| 8 | `testing.md`, `unsafe-rust.md`, `sqlx.md`, `slint.md` (citations only) | | 2, 1, 1, 0 | 0 citations in all four trees |
| 9 | `slint.md`'s font bullet | 141 / 10,214 | | 145 / 10,438, the bullet wrapped at 100 |

## Phases

### Phase 1: `audio-stack.md`
Status: done.

- [x] **1. The rule and its eight citations**, plus `scripts/patch_vazirmatn.py:45`'s
  `slint-pitfalls.md` citation, which slint-pitfalls' sweep missed because it covered only
  `crates/`.
  - **Stale, fixed or cut:**
    - `decode_file` no longer exists. The decode hoisted out of the decks lock is
      `FileDecoder::open`.
    - "`Loading` clears Discord presence" was false: `discord/model.rs` holds the card. Only the
      MPRIS half survives (it reports `Loading` as Stopped).
    - "`play_media`/`begin_crossfade` re-apply the speed": neither touches the visualizer's speed
      cell, and `PlaybackEngine::set_speed` is its only writer.
    - The crossfade settings rows are gated on `if root.show-crossfade-details` now, and
      `playback-section.slint` argues the `if` itself. Cut.
  - **No comment's stated reason was false.** `melodia-engine/src/lib.rs` says the engine names
    "neither cpal nor the network" while its manifest lists cpal, but only as a dev-dependency.
  - **Verified against source:**
    - Symphonia 0.6.1: no Opus decoder crate, and Ogg demuxes Opus. isomp4 parses `edts` but sets
      no delay or padding, and mkv parses `DiscardPadding` that nothing reads.
    - alsa 0.11.0: `PCM::delay` is `snd_pcm_delay`, and alsa-lib's header has `S24_LE` low in its
      32 bits.
    - alsa-sys 0.4.0: it probes pkg-config 0.3.34, which refuses a cross build.
    - cpal 0.18.2: the output timestamp carries callback and playback times, the ALSA host pulls
      whole periods, and a WASAPI device id is `IMMDevice::GetId`.
    - stream-download 0.24.4: bounded storage pauses the writer when the reader falls behind.
  - **Not verified:** WASAPI's 24-in-32 being high-justified, and the "near 5 dB/octave" display
    target now in `spectrum.rs` (the old rule's figure, carried over and never measured).
  - **Handed on:** radio.md's "D8" (Phase 5.1), symphonia.md's "a count against a window"
    (Phase 6), `deploy-coverage.yml:32`'s citation (Phase 4), the PR template (Open questions).

### Phase 2: `desktop-shell.md`
Status: done.

- [x] **2. The rule and its citations**, one commit.
  - **Result:** sections now Window chrome, the native miniplayer's frame, the shell and its edge,
    File drops, Tray and media keys, and Shutdown (kept for audio-stack's citation). Cut as carried
    at their anchors: the file-manager open path, single-instance, the tray façade, the
    always-on-top methods, the restore caption's floor, first-launch KDE defaults.
  - **Stale, fixed:** `resize-border-width`'s handler no longer "compares the logical band with a
    physical cursor". Slint 1.18.1's `handle_cursor_move_for_resize` scales the band by
    `scale_factor()`, where 1.16.1 compared it raw. Only the square corner survives.
  - **Comment whose reason went false (Decision 3):** `frameless_tests.rs:214`'s doc on
    `the_shell_leaves_every_resize_press_to_the_ring`, which said the same and promised "a thinner
    edge on a scaled display". Rewritten to the corner alone.
  - **Code diff:** 18 comment lines out, 18 in, 32 characters fewer.
  - **Verified against source:**
    - i-slint-backend-winit 1.18.1: windows built from the active event loop
      (`create_inactive_windows`), so `with_winit_window` is `None` before; `no-frame` re-applied
      live through `set_decorations`; minimize and maximize forwarded only when the cached flag
      differs; transparent except on macOS (`wants_transparent`) and Windows with a menubar.
    - i-slint-core 1.18.1: `set_minimized`/`set_maximized` only set the item's flag.
    - The winit fork: `focus_window` and `is_visible` unsupported on Wayland, macOS
      `drag_resize_window` returns `NotSupported`; `winit/README.md` matches the four commits.
    - souvlaki 0.8.3: the Windows backend `expect`s an `HWND`.
  - **Not verified:** the Win32 claims (frame steps, invisible borders, the parked loop, the
    tray ghosting), which need the Windows machine; all are argued at their anchors.
  - **Covers** window chrome and winit, the tray, media keys, always-on-top, and the force-exit
    shutdown.
  - **Code citations (8 in 6 files):**
    - `crates/melodia-app/src/services/settings/interface.rs:98`;
    - `crates/melodia-ui/ui/app-window.slint:439` and `:491`;
    - `crates/melodia-ui/ui/components/custom-titlebar.slint:9`;
    - `crates/melodia-ui/ui/globals/drag.slint:109`;
    - `crates/melodia-ui/ui/globals/shell.slint:12` and `:19`;
    - `crates/melodia/tests/file_drop.rs:4`.
  - **Cited by, keep findable:**
    - `audio-stack.md` names its **Shutdown** section by name, so keep that heading;
    - `slint-pitfalls.md:412`, at the OS file drop that Rust routes;
    - `ui-patterns.md:42`, at `FileDropBanner` and a file dragged in from the OS;
    - `CLAUDE.md:14`'s table row.
  - **Dependencies to read:** the vendored winit fork is `winit/` in the repo (winit 0.30.13 plus
    four commits), not the registry.

### Phase 3: `library-data.md`
Status: done.

- [x] **3. The rule and its citations**, one commit.
  - **Result:** the six sections kept, each entry cut to the contract and its anchor. The sweep
    now points at `queries::artwork::ARTWORK_COLUMNS`, which already argued the six columns and
    the three that bite. Cut as carried at their anchors: the chunk and bulk-trigger mechanics,
    the grace window and `retire_released`, the first-launch folder and nested-folder absorb, the
    projection column counts, `TagEditRow`'s missing multi-valued fields, `DurationMs` in seconds,
    the full rating index, lyrics living in the file. The mid-session `missing_artwork` reconcile
    went as root `CLAUDE.md`'s.
  - **Stale:** none. Every qualified name resolves to its definition; `20260910000000` is still
    the newest `tracks_fts` rebuild and the index still has eight columns; only
    `bind_track_columns` writes the five searchable columns.
  - **No comment's stated reason was false.**
  - **Code diff:** 10 comment lines out, 10 in, 111 characters more.
  - **Verified against source:** SQLite in libsqlite3-sys 0.30.1 refuses `ALTER` on a virtual
    table ("virtual tables may not be altered").
  - **Still landing:** `radio.md:42` and `:96` (the sweep, the ledger) and `ui-patterns.md:216`
    (`search_name`).
  - **Covers** scan ingest, the two change channels, track projections, ratings, tag
    write-through, M3U8, and smart playlists.
  - **Code citations (6 in 6 files):**
    - `crates/melodia-core/src/entities/tags.rs:143`;
    - `crates/melodia-core/src/entities/track.rs:152`;
    - `crates/melodia-core/src/utils/self_writes.rs:8`;
    - `crates/melodia-ui/ui/components/dialog/smart-playlist-editor-body.slint:101`;
    - `crates/melodia-ui/ui/globals/dialog-forms.slint:38`;
    - `crates/melodia/tests/cross_tier.rs:256`.
  - **Cited by, keep findable:**
    - `radio.md:42`, at the artwork sweep;
    - `radio.md:96`, at the ledger both halves of the logo cache turn on;
    - `ui-patterns.md:216`, at each entity's `search_name`;
    - `CLAUDE.md:12`'s table row.

### Phase 4: `ci-packaging.md`
Status: done.

- [x] **4. The rule and its citations**, one commit.
  - **Result:** the four sections kept. Cut as carried at their anchors: the job-DAG argument,
    the aggregate's and the skip matrix's mechanics, the audit flags, rust-cache's key suffix,
    what the headless shim leans on (root `CLAUDE.md`), the frame-budget lesson
    (`CONTROL_OP_BUDGET`), `components`, `%F` over `%U`, the DEP-5 and Policy 12.5 arguments,
    `licenses/` against `Files: *`, the font pin and the toolchain bump (both root `CLAUDE.md`),
    and the cargo-wix and upload-glob stories (`release-build.yml`). Cut as history: the two-phase
    build and every memory and build-time figure.
  - **Stale, fixed or cut:**
    - "48 test binaries": `cargo metadata` counts 60 harnesses (47 integration, 12 lib, 1 bin).
    - "C13" resolves to nothing, like radio.md's "D8".
    - `CODEOWNERS` protects `.github/actions/` too, not only the workflows.
    - "the Ko-fi stanza's argument over five more files": four texts sit under the glob.
    - "only `windows-2025` is documented to keep" naming that image: the runner-images README
      lists all three labels on one image and says nothing about which lasts. Dropped.
  - **Comments with a stale fact (Decision 3), rewritten count-free:** `pr-validation.yml`'s
    "42 test binaries" and "three `include_*!`d payloads" (four today).
  - **Code citations swept (4):** the named one, plus three the grep couldn't see:
    `.cargo/audit.toml:2` (outside the four trees) and two unnamed "the rule file" pointers in
    `pr-validation.yml` (the denylist note and the `test-windows` note).
  - **Code diff:** 10 comment lines out, 10 in, 121 characters more.
  - **Verified against source:**
    - Swatinem/rust-cache 2.9.1: OS and arch appended after `shared-key`, `CARGO_INCREMENTAL=0`
      exported, `CARGO*` env hashed into the key.
    - dorny/paths-filter 4.0.2: `predicate-quantifier` defaults to `some`.
    - fkirc/skip-duplicate-actions 5.3.2: `paths_filter` is an input.
    - Every pinned action is `node24` or a composite (`attest-build-provenance` over
      `actions/attest`, `upload-pages-artifact` over `upload-artifact`, taiki-e's two over bash
      and `actions/cache`). No `uses:` floats.
    - runner-images: Defender real-time monitoring off and `C:\` and `D:\` excluded.
    - cargo-deb 3.7.0: `has_copyright_metadata` reads the first ten lines for any DEP-5 key.
    - `ring`, `aws-lc-sys`, `libsqlite3-sys` and `blake3` are all in `Cargo.lock`.
  - **Not verified:** the Policy 12.5 footnote and lintian's tag coverage (no lintian here), and
    `cargo wix`'s single-member auto-select at the git pin (argued at its step, not reread).
  - **Coverage regex:** redundant, removed after the phase; see Open questions 4.
  - **Code citation (1):** `.github/workflows/deploy-coverage.yml:32`, a YAML `#` comment.
  - **Cited by, keep findable:** the most-cited rule.
    - `testing.md:15`, at the gate and the coverage flags;
    - `CLAUDE.md:18`'s table row;
    - `CLAUDE.md:78`: grep `release*.yml` and the packaging scripts before adding a member;
    - `CLAUDE.md:124`: every format ships `licenses/`;
    - `CLAUDE.md:140`: `%F` on four `.desktop` sources plus a fifth `sed`;
    - `CLAUDE.md:191`: the list of what bites;
    - `docs/adr/0006-agpl-and-what-artifacts-owe.md:55` and
      `docs/adr/0031-five-package-formats.md:34`. Both are accepted ADRs, which are never
      rewritten (Decision 6).
  - **Its line 22 points at `updater.md`**, which Phase 5.3 must keep landing.
  - **Dependencies to read:** GitHub Actions aren't in the registry. Check claims against the
    workflow files and the pinned SHAs, and say which couldn't be checked locally.

### Phase 5: `radio.md`, `visualizer.md`, `updater.md`
Status: done.

- [x] **5.1 `radio.md`**, one commit.
  - **Result:** the five sections kept. Cut as carried at their anchors: the facade walks' own
    arguments (`radio_facade.rs`), `fold_disabled_nav_index`'s placement, the `local_*` and
    logo-answer arguments (the two migration headers), id reuse (`detail_tests.rs`), the feed
    thread's inline name (its spawn), the `stream-download` pin (root `Cargo.toml`), a paused
    station dropping its socket (`audio-stack.md`), the batch star and remove, and the leave's
    card-selection clear (`card_selection.rs`, `ui-patterns.md`).
  - **Stale, fixed or cut:**
    - "D3", "D6", "D8" and "D9" were decision numbers from `docs/plans/INTERNET_RADIO.md`,
      deleted in `4124c530`. D8 became ADR 14 and D15 ADR 15; the rest never became ADRs.
    - The radio migration's header was said to argue "no `AUTOINCREMENT`, no secondary indexes".
      It never mentions `AUTOINCREMENT` and argues the one index it creates,
      `idx_radio_stations_stream_url`.
    - "Four" walks holding the switch: `radio_facade.rs` has five, counting
      `the_facade_builds_no_client_of_its_own`.
    - `id == 0` was said to be spelled by hand once below `melodia-views`. The engine's
      `PlayerState::to_persisted` spells it too.
  - **Dangling plan labels in code, fixed (comment-only):** `detail_tests.rs:101`'s "(D6)",
    `radio_facade.rs:46`'s "D15's switch", and `state_tests.rs:1491`'s "D11:", the last outside
    this rule's trees but from the same deleted plan.
  - **No comment's stated reason was false.**
  - **Code diff:** 12 comment lines out, 13 in, 44 bytes more.
  - **Verified against source:** stream-download 0.24.4's defaults pull `reqwest/default-tls`, and
    `new_http` reaches `Client::create`, a `LazyLock` over a bare `reqwest::Client::new`.
  - **Not verified:** nothing left open.
  - **Covers** the directory facade and its off switch, the two station tables, the stream path's
    ring and feed thread, and the numbers both trees spell.
  - **Code citations (7 in 5 files):**
    - `crates/melodia-core/src/entities/radio.rs:18`;
    - `crates/melodia-ui/ui/components/dialog/add-station-body.slint:9`;
    - `crates/melodia-ui/ui/globals/radio.slint:73`;
    - `crates/melodia-ui/ui/models.slint:196`, `:378` and `:383`;
    - `crates/melodia-ui/ui/settings.slint:196`.
  - **Cited by, keep findable:**
    - `audio-stack.md`'s live-source section, at the facade, the off switch and the station tables;
    - `CLAUDE.md:17`'s table row;
    - `CLAUDE.md:82`, at `radio/` as the one door;
    - `CLAUDE.md:83`, at what crosses out of `player/`.
- [x] **5.2 `visualizer.md`**, one commit.
  - **Result:** the four sections kept, one or two entries each: the arm split and its sole
    writer, the `analyzing` guard and the gate pins, the bars' ask, the picker list by position
    and the hosts' `dismiss()`. Cut as carried at their anchors: the ring re-arm and first
    allocation (`VisualizerShared::set_enabled`), the session's buffers (`Session`,
    `wire_set_active`), the `pulse` deferral, the strip's dimensions and the cover slot's slack
    (`now-playing-view.slint`), the Mirrored anchor and its half-height (`write_bar_path`), the
    33 ms interval (`visualizer-strip.slint`, `VISUALIZER_DECAY`), the trace's cost
    (`waveform.rs`), the picker's popup, "Off" and trigger state (`visualizer-picker.slint`) and
    the rows' container (`menu-surface.slint`). Cut as trivia: the 56 px fallback, "mirror" as an
    overloaded word.
  - **Stale:** none in the rule. Every name resolves, and the two Slint internals it named
    (`solve_box_layout`, `layout_items`) are in i-slint-core 1.18.1's `layout.rs`.
  - **Comment whose reason went false (Decision 3):** `visualizer-flyout.slint`'s header said the
    flyout lives inside a menu's `PopupWindow` beside its column, like the sleep flyout.
    `VisualizerPicker` is its only mount, and it fills that popup. Rewritten.
  - **Code diff:** 13 comment lines out, 11 in, 99 bytes fewer. Clippy clean.
  - **Verified against source:**
    - i-slint-core 1.18.1: `solve_box_layout` falls through to `layout_items` once the preferred
      sizes stop fitting, and `layout_items` shrinks from `pref` and positions from the start.
    - i-slint-renderer-femtovg 1.18.1: with a rendering notifier set, every frame flushes right
      after the clear (`lib.rs:246`).
  - **Not verified:** nothing left open.
  - **Covers** the visualizer's UI half: arming, the tick's gates, the strip and its pickers.
  - **Code citations (7 in 6 files):**
    - `crates/melodia-app/src/library/playback.rs:512`;
    - `crates/melodia-ui/ui/components/now-playing/spectrum-bars.slint:6` and `:11`;
    - `crates/melodia-ui/ui/components/now-playing/visualizer-strip.slint:27`;
    - `crates/melodia-ui/ui/globals/audio.slint:43`;
    - `crates/melodia-ui/ui/views/now-playing-view.slint:158`;
    - `crates/melodia-views/src/ui/visualizer/mod.rs:3`.
  - **Cited by, keep findable:** `audio-stack.md`'s visualizer section ("the UI half"),
    `CLAUDE.md:16`'s table row and `CLAUDE.md:117`.
  - **Its line 22 cites `audio-stack.md` for the DSP**, which still lands.
- [x] **5.3 `updater.md`**, one commit.
  - **Result:** the three sections kept, plus an opening pointer at
    `crates/melodia/tests/updater_contract.rs`, which already holds four of the rule's couplings
    (the trusted-comment fields, the prehashed signing step, the platform keys, the schema
    default). Cut as carried at their anchors: `manifest=true`'s domain separation
    (`verify_manifest_bytes`), the fail-closed order (`github.rs`), the schema bump and
    `critical` (`manifest.rs`, the test), the MSI's mechanics, exit codes and shortcut `Icon`
    (`main.wxs`, `swap::msiexec_outcome`), the portable-extract short-circuit
    (`system_install`), `desktop_integration`'s whole entry (its module doc; the `include_*!`
    paths fail the build), the attestation step and verify command (`release.yml`, `README.md`),
    and the per-arch linuxdeploy SHAs (`build-appimage.sh`). Cut as history: v0.8.0's missing deb
    entry and v0.9.0's broken refresh (both still told at their anchors), and the "fold into
    `actions/attest`" to-do, which is a suggestion rather than a contract.
  - **Stale, fixed:**
    - "`is_system_install()` ... the update is real, only the mechanism is the package manager's,
      so the check survives": `probe` returns `false` for RPM, DEB and MSI, so a system install is
      one no in-app path reaches (a root-owned install rpm and dpkg don't own, a portable Windows
      copy). Boot skips the daily task there; only the manual check survives.
    - "bumping it means bumping ... `--manifest-schema-version` and the CI invocation": no
      workflow passes one. The script's default is the whole of it, which the contract test pins.
  - **Comments whose reason went false (Decision 3), rewritten:** `updater.slint`'s
    `system-managed` note named "(RPM, MSI, Homebrew)", two of them installs that keep Download;
    `update-section.slint`'s two notes and `features-panel.slint`'s said "package-managed" and
    "the package manager suppresses the task", where an RPM install keeps both; `swap.rs`'s
    `elevate_swap_via_pkexec` doc credited `is_system_install()` with keeping RPM and deb off the
    path, which `install_method_for_key` does, and said "only a tarball" where a root-owned
    AppImage arrives too.
  - **Code diff:** 11 comment lines out, 11 in, 10 bytes more. Clippy caught a bare `AppImage`
    (`doc_markdown`) in the rewritten `swap.rs` doc, fixed by hand; see Open questions 1.
  - **Verified against source:**
    - dirs 7.0.0: `cache_dir()` is `data_local_dir()`, `FOLDERID_LocalAppData` on Windows.
    - windows-sys 0.61.2: `ERROR_SUCCESS_REBOOT_REQUIRED` 3010, `ERROR_SUCCESS_REBOOT_INITIATED`
      1641, `ERROR_INSTALL_USEREXIT` 1602.
  - **Not verified:** Restart Manager's event 10010 and the elevated SID (Win32 docs, argued at
    `main.wxs`).
  - **Found, not this plan's (no code changes):**
    - The Available toast's Install action (`notifications.rs`, `InstallUpdate`) calls
      `MelodiaUpdater.install()` with no `system-managed` gate, so a manual check on a system
      install offers the install the panel hides. A root-owned tarball then reaches `pkexec mv`.
    - The panel's hint reads "Use sudo dnf update melodia" on any system-managed Linux install,
      including a hand-dropped `/opt/` tarball no package manager owns.
    - `swap::windows_swap` is unreachable in production: a Windows key is always `windows-*-msi`,
      which resolves to `WindowsMsi`.
    - `PLATFORM_PATTERNS` carries two `windows-*-zip` keys no slot builds and no client asks for,
      which `updater_contract.rs`'s `MANIFEST_KEYS` (12) pins.
  - **Covers** install methods, manifest signing, and the release matrix.
  - **Code citation (1):** `crates/melodia-core/src/utils/exe.rs:8`.
  - **Cited by, keep findable:**
    - `ci-packaging.md:22`, at the release matrix, so keep the `## Release matrix` heading;
    - `CLAUDE.md:15`'s table row;
    - `docs/adr/0019-updater-trust-boundary-is-the-repo.md:45`, an accepted ADR.

### Phase 6: `symphonia.md`
Status: done.

- [x] **6. The rule**, one commit. It has no code citations.
  - **Result:** three sections, Opening and decoding, Seeking, and Gapless, each entry a pointer
    at the `source/` module holding it plus what Symphonia 0.6.1 does underneath. The gapless
    section is now one line per container (`aac_trim`, `mkv_trim`, `opus`) and the
    `Track::delay` comparison, which no one module holds. Cut as history: the 0.5 probe and
    `enable_gapless`, the two-majors story, symphonia#274's misreading. Cut as not this tree's:
    "reuse buffers" (`fill` already does) and metadata-only reads (tags are lofty's).
  - **Stale, fixed or cut:**
    - The decode-loop sketch was 0.5's: 0.6.1's `next_packet` returns `Ok(None)` at the end, and
      `ResetRequired` ends the source here rather than re-reading. Now a pointer at `decode::fill`.
    - "Sample Format Conversion" named `SampleBuffer` and `copy_interleaved_ref`, neither in
      0.6.1. A packet is a `GenericAudioBufferRef`, flattened with `copy_to_vec_interleaved`.
    - "default track may be video": 0.6's `default_track` takes a `TrackType`. The real trap is
      a null-codec track winning, which `decode::audio_track` argues.
    - `player::decode`, `player::aac_trim` and `player::mkv_trim` live under `player::source::`.
    - The intro put "the two decks" in `output/`; those are `playback::decks`, `output` holding
      the voices. The intro now names `audio-stack.md` alone.
    - "a count against a window" resolved to nothing in either trim module or `audio-stack.md`.
      Dropped.
  - **Comment whose reason went false (Decision 3):** `decode::open`'s doc said the hint is one
    "0.6 scores rather than trusts". 0.6.1's `Probe::next` takes it as `_hint` and never reads
    it. Rewritten to "which 0.6 ignores, scoring instead"; `stream_decode.rs:61` already said so.
  - **Code diff:** 1 comment line out, 1 in, 1 byte fewer. Clippy clean.
  - **Verified against source (symphonia 0.6.1):**
    - `Probe::probe` and `next` never read the hint; `find_reader` scores each tier's readers
      whose marker matches.
    - `AudioDecoderOptions::gapless` defaults to `true`; only `symphonia-bundle-mp3` and
      `symphonia-codec-vorbis` read it.
    - `FormatReader::seek` asks every consuming decoder to be reset. PCM, ADPCM, ALAC and FLAC
      `reset` do nothing; MP3 rebuilds `State`; AAC zeroes each ICS `delay` buffer; Vorbis
      clears its overlap buffer.
    - MP3 starts `next_packet_ts` at `-delay` and CAF `start_pts` at `-priming_frames`.
    - Ogg's Opus mapper overrides no `absgp_to_ts` (the trait's identity) and its packet parser
      reports zero discard, so no packet trim and a start at the granule less the packet
      durations, zero for a stream whose granule counts the pre-skip. This settles the claim
      Phase 1 left open.
    - Matroska subtracts `CodecDelay` from block timestamps (`lacing.rs:97`) and parses
      `DiscardPadding` into a field nothing reads.
  - **Not verified:** nothing left open.
  - **What it is:** a per-crate practice rule, named in `CLAUDE.md`'s table only in the "rest" row.
    Nothing cites it by file name.
  - **It cites `audio-stack.md` three times** (`:12`, `:92`, `:96`), and all three still land after
    Phase 1.
  - **Its `:96` names a comparison audio-stack never held**: `aac_trim` against `mkv_trim` as "a
    count against a window". Find what it meant in the two modules' docs, or drop the phrase.
  - **Dependencies to read:** symphonia 0.6.1 and opus-pure 0.2.2. Phase 1 already verified:
    - no Opus decoder upstream;
    - isomp4 sets no delay or padding;
    - mkv's `DiscardPadding` is unread;
    - Ogg's Opus mapper sets the delay from `pre_skip` (`mappings/opus.rs:62`).

    Its claim that the mapper leaves `absgp_to_ts` at identity is still unchecked.

### Phase 7: Delete `diagnostics.md`
Status: done.

- [x] **7. Move its one decision, then delete it**, one commit.
  - **Result:** the declined console-attach now sits in the `--logs` comment, one line longer
    (2 comment lines out, 3 in). Every other part was found at its anchor on recheck, including
    what the 2026-10-27 list didn't name: `set_verbose`'s `RUST_LOG` precedence, the level guide
    in `logging`'s module doc, `install_hook`'s chaining, the plain-`fs` write in the hook, and
    `take_unseen`'s once-per-crash gate. `rules_globs.rs` passes at 176 paths against 182.
  - [x] **Move the declined decision.** Write the declined Windows console-attach into the comment
    on the `--logs` branch in `crates/melodia/src/main.rs` (lines 36 to 38). Its reason: one
    diagnostic is not worth another Win32 FFI site. Comment lines only.
  - [x] **Confirm the rest is carried.** Checked 2026-10-27:
    - the local timestamps: `crash_report.rs`'s `//!` (lines 11 and 12) and `diagnostics.rs:34`;
    - the four decisions, each at its definition: `logging::install`, `log_files` and
      `newest_first`, `head_of` and `tail_of`, and `error::describe` in `CLAUDE.md`;
    - credentials: `CLAUDE.md`'s "Never log a credential";
    - the sweep: `CLAUDE.md`'s crash-report sweep bullet;
    - `--logs`: `CLAUDE.md`'s `--version` bullet and `main.rs`.

    Recheck before deleting.
  - [x] **Delete `.claude/rules/diagnostics.md`.** `rules_globs.rs` then sees 176 paths, against 182
    and its floor of 100 (`MIN_PATHS`).
  - [x] **Remove `CLAUDE.md`'s two pointers:**
    - the table row (line 19);
    - the end of the `services/` bullet (line 85): "The diagnostics trio (...) answers to a bug
      report and has its own rule: `.claude/rules/diagnostics.md`."

    Nothing else names the file. Grep `README.md`, `CONTRIBUTING.md`, `docs/` and the other rules
    again first.

### Phase 8: Sweep the citation-only rules
Status: done.

- [x] **8. Their citations**, one commit. Steps 1, 3 and 5 of Every rule only. The rules themselves
  aren't compressed.
  - **Result:** the fresh grep found the same four and nothing unnamed. The two `Cargo.toml`
    comments naming `.claude/rules` describe what `rules_globs.rs` reads, not a reason, and stay.
    Each site now states its reason: `http.rs` that a mock framework is a workspace-wide choice
    and the workspace runs none; `cross_tier.rs` that the corpus walks live in this crate so a
    per-crate `cargo test` stays about that crate; `ENV_LOCK` drops the pointer, the `SAFETY`
    comment in `with_env_set` already arguing why mutation needs the lock; and `init_database`
    names `BEGIN IMMEDIATE` as what a contended writer takes to fail fast.
  - **Code diff:** 13 comment lines out, 12 in (`ENV_LOCK`'s doc one shorter, its two em dashes
    gone with the sentence), 150 bytes fewer. Clippy clean.
  - **`testing.md` (2):** `crates/melodia-testkit/src/http.rs:13` and
    `crates/melodia/tests/cross_tier.rs:9`, a test module doc.
  - **`unsafe-rust.md` (1):** `crates/melodia-testkit/src/env.rs:7`.
  - **`sqlx.md` (1):** `crates/melodia-store/src/database/mod.rs:316`, where "DEFERRED rather than
    `sqlx.md`'s `BEGIN IMMEDIATE`" leans on the rule for its reason. State the reason inline.
  - **`slint.md` (0).** A bare `slint.md` grep hits `crates/melodia-artwork/src/lib.rs:3`, but that
    is ADR 0033's filename (`0033-artwork-depends-on-slint.md`), and it stays.

### Phase 9: Fix `slint.md`'s font advice
Status: done.

- [x] **9. The Images & Fonts bullet**, one commit. Markdown only.
  - **Result:** the bullet now says fonts are imported, never registered from Rust: a `.slint`
    `import` of the face embeds it, `app-window.slint` importing the three text faces and naming
    them in `default-font-family`, `material-icon.slint` the two icon faces. It points at root
    `CLAUDE.md`'s Fonts and Icons bullets for the faces and the patching.
  - **Worse than the plan said:** Slint 1.18.1 has no `slint::FontFile` at all. The only
    registration is `RendererSealed::register_font_from_{memory,path}`, internal, which the
    generated code calls once per imported face: five `register_font_from_memory` calls in
    `app-window.rs`, which is also what confirms the faces are embedded.
  - **What it says** (line 119): register fonts at startup with `slint::FontFile::load_from_path`
    so `default-font-family: "Inter"` resolves.
  - **What the tree does:** `app-window.slint` imports the TTFs statically
    (`import "assets/fonts/vazirmatn/Vazirmatn-*.ttf"`, lines 2 to 4) and sets
    `default-font-family: "Vazirmatn"` (line 145). Nothing names `FontFile`.
  - **Root `CLAUDE.md`'s Styling ▸ Fonts bullet already argues the faces and the patching**, so
    the rule's bullet states the static import and points there, rather than becoming a second
    copy.

### Phase 10: Retire the plan
Status: not started.

- [ ] Confirm Progress is complete and every phase landed.
- [ ] Check that no decision recorded only here goes unrecorded once this file is deleted.
- [x] Offer the `CLAUDE.md` and `README.md` edits the phases leave, if any. All three made on
  2026-10-28; the radio row names "the stream's prohibitions" in place of the ring and feed
  thread, that being what its stream section now holds. No `README.md` edit was owed.
  - From Phase 2: root `CLAUDE.md`'s UI Structure bullet still says Slint's `resize-border-width`
    handler "compares a logical band with a physical cursor", stale since Slint 1.18. Drop that
    clause; the square corners alone carry the argument `app-window.slint:140` points to.
  - From Phase 5.1: root `CLAUDE.md:17`'s `radio.md` row still lists "the stream path's ring and
    feed thread", which the rule now hands to `audio-stack.md`'s live source. Drop that clause.
  - From Phase 5.3: root `CLAUDE.md:137` says "Inside the updater use `updater::install_target`",
    which resolves nowhere: it is `install_kind::install_target`, in `melodia-platform`.
- [ ] Delete this file.

## Cross-cutting

- **Phases hand findings forward.** Anything a phase finds for a later rule goes into that phase's
  notes above, as Phase 1's did.
- **A compressed rule can break a later phase's citation.** Phase 1 made `audio-stack.md` cite
  `desktop-shell.md`'s Shutdown, and Phase 4 holds `updater.md`'s release-matrix pointer. So step 1
  greps the citations afresh every phase, rather than trusting the lists above.
- **No hand pass, no release build and no RSS run.** Nothing a user sees changes.

## Open questions

1. **Resolved: any `.rs` edit runs clippy, comments included; markdown-only edits don't**
   (Kenan, 2026-10-28). A scratch crate on the workspace lint table showed 10 lints failing a
   comment-only edit under `-D warnings`: eight on `///`/`//!` prose (`doc_markdown`,
   `doc_lazy_continuation`, `doc_overindented_list_items`, `doc_link_with_quotes`,
   `tabs_in_doc_comments`, `empty_docs`, `empty_line_after_doc_comments`,
   `suspicious_doc_comments`) and two on plain-comment edits (`four_forward_slashes`, rustc's
   `unused_doc_comments`). Phase 5.3's `swap.rs` doc was a live case. A warm run costs 3 to 6 s.
   Outside clippy's reach: a `.slint` `// Translators:` line, which `translations.rs` reads, and
   broken intra-doc links, which only `cargo doc` checks and CI never runs. Decision 7's "no
   clippy" is superseded.
2. **Resolved: the PR template no longer sends contributors to `.claude/rules/`** (Kenan,
   2026-10-28). Its docs checklist names `CLAUDE.md` and `README.md`; `CONTRIBUTING.md` and
   `README.md` never named the rules.
3. **Resolved: every rule is wrapped at 100 columns** (Kenan, 2026-10-28: the long lines hurt
   readability). Not only the three named here: ten rules held 175 lines over 100, now none
   outside 12 table rows (`code-style.md` 2, `unsafe-rust.md` 10), which markdown can't wrap.
   Line breaks only: every frontmatter byte-identical, every file's text identical with whitespace
   collapsed, save one `#` continuing a TOML comment in `rust-performance.md`'s release-profile
   block, and a `\` continuing `code-style.md`'s `cloc` command. Root `CLAUDE.md` took the same
   treatment (109 lines over 100, the longest 2,905): what remains is its 9 table rows and two
   code spans too long for any line, the `app-window.slint` layout sketch and the `msginit`
   command, each now on a line of its own.
4. **Resolved: coverage's `--ignore-filename-regex` was redundant, and is gone.** cargo-llvm-cov
   drops its whole target dir by default, `melodia-ui`'s `OUT_DIR` included (0.8.7 in CI, 0.9.1
   locally, and as far back as 0.5.0). Checked on 2026-10-28 with an instrumented `melodia-views`
   run: the defaults alone and CI's regex gave byte-identical summaries, and switching the
   defaults off brought `app-window.rs` back, so the file was in the map. `22006716`'s message
   claiming otherwise was wrong.

## Verification

All of these run in zsh from the repo root, with `r` set to the rule's name and `f=.claude/rules/$r.md`.

- **Size, and lines over 100 columns.** GNU awk counts characters, not bytes:
  ```bash
  git show HEAD:$f | wc -lc; wc -lc < $f
  awk 'length($0) > 100' $f | wc -l
  ```
- **Frontmatter unchanged:**
  ```bash
  fm() { awk '{ print } NR > 1 && /^---$/ { exit }'; }
  cmp <(git show HEAD:$f | fm) <(fm < $f) && echo identical
  ```
- **Citations left in code**, all four trees:
  ```bash
  git grep -n -F "$r.md" -- crates scripts .github packaging
  ```
- **Comment-only check.** It prints every changed line that is neither a comment nor blank. A
  Python docstring line shows up there, and is read by eye:
  ```bash
  git diff HEAD -U0 -- '*.rs' '*.slint' | command grep -E '^[-+]' | command grep -vE '^(\+\+\+|---) ' \
    | cut -c2- | command grep -vE '^[[:space:]]*(//|$)'
  git diff HEAD -U0 -- '*.py' '*.sh' '*.yml' '*.toml' | command grep -E '^[-+]' \
    | command grep -vE '^(\+\+\+|---) ' | cut -c2- | command grep -vE '^[[:space:]]*(#|$)'
  ```
- **No added line over 100 columns, and no em dash added:**
  ```bash
  git diff HEAD -U0 | command grep -E '^\+[^+]' | awk 'length($0) - 1 > 100'
  git diff HEAD -U0 | command grep -E '^\+[^+]' | command grep -c '—'
  ```
- **Identifier check.** It prints each backticked name the tree no longer holds. Names the rule
  itself says were deleted show up, and so do wildcards like `*_SUPPORTED`. Check path tokens by
  hand with `find crates -path '*<token>'`, since the rules spell many of them tier-relative:
  ```bash
  command grep -o '`[^`]*`' $f | tr -d '`' | command grep -oE '[A-Za-z_][A-Za-z0-9_]{2,}' | sort -u \
    | while read -r w; do git grep -qw -F "$w" -- crates scripts .github docs packaging || echo "missing: $w"; done
  ```
- **Formatting:** `cargo fmt --all --check`, plus `rustfmt --edition 2024 --check <file>` for each
  file Decision 7 names.
- **Lints**, whenever a `.rs` file changed:
  `cargo clippy --all-targets --locked --workspace -- -D warnings`.

## Tools

- **Print the doc comment above an item:**
  ```bash
  docof() {  # docof <file> <regex>
    local line; line=$(command grep -n -E "$2" "$1" | head -1 | cut -d: -f1)
    [ -n "$line" ] || { echo "no match: $2"; return; }
    awk -v end="$line" 'NR < end { if ($0 ~ /^[[:space:]]*(\/\/|#\[)/) buf = buf $0 "\n"; else buf = "" }
      NR == end { printf "%s", buf; print; exit }' "$1"
  }
  ```
- **Dump every module doc under a directory**, to see what the anchors already carry:
  ```bash
  for m in $(find <dir> -name '*.rs' -not -path '*/tests/*' | sort); do
    command grep -q '^//!' $m && { echo "### $m"; command grep '^//!' $m | cut -c5-; }
  done
  ```
- **Shell notes.**
  - `grep` is ugrep, so script with `command grep` or `git grep`.
  - `ls` is colourised, so `$(ls ...)` hands a loop escape codes. Use
    `find ... -printf '%f\n'`.
  - In zsh:
    - quote globs, as in `--include='*.rs'`;
    - a word that starts with `=` expands as a command path, so quote a separator like
      `echo '-- x'`;
    - a variable holding a command and its arguments doesn't word-split, so wrap it in a function.
