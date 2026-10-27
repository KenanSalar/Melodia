# Plan: Comments down to about a quarter of the code

Status: **in progress**, Phase 8.3 next · Owner: Kenan · Created: 2026-10-25 · Issue #125, branch `docs/trim-comments`

> **Measured 2026-10-25** against `32a188b1` (`main`), with the `cloc` commands under Verification. Every figure below comes from that run. Each phase measures its slice again before it starts, since `main` can move under the branch.

Working doc. Delete when every phase has landed and the target is in `.claude/rules/code-style.md`.

## Context

**What you see today.**
- **In places the prose outweighs the code.** Production Rust has 30,355 comment lines against 71,751 lines of code (0.42). The Slint tree has 9,204 against 23,236 (0.40).
  - `melodia-core`'s `utils/` is at 0.90 and the Slint `globals/` at 0.84.
  - `globals/radio.slint` holds 150 comment lines over 85 of code, and `ui/grid_prewarm.rs` 189 over 168.
- **Much of it is one of three things** (#125):
  - changelog: the bug story, what the code used to do, what shipped before;
  - narration: a step-by-step walk through code that's right there, or the name and signature said again;
  - second copies: an argument made in a doc comment and again in `CLAUDE.md`, a rule or an ADR.
- **Every one of those lines is prose someone has to keep true**, and a stale one is worse than none.

**Why now.** #120's phases 1 to 8 moved and deleted code, and all of it has landed, so the code is where it's going to stay.

**Moved since the issue was written.** Rust is at 0.42 (the issue had 0.44) and `melodia-views` at 0.39 (0.43). The binary now comes before views in the worst-first order.

**Outcome.**
- **Kept, shorter but never removed:**
  - `// SAFETY:` comments;
  - the why behind ordering, lock order, platform quirks and magic constants;
  - couplings across trees, like an index order a `.slint` list follows;
  - anything `CLAUDE.md`, a rule, an ADR or a test points at, condensed with the pointer still true;
  - the Slint tree's 36 `// Translators:` notes, byte for byte.
- **Removed:**
  - the changelog register ("used to", "no longer", "previously");
  - narration, and restated names and signatures;
  - the second copy of anything argued elsewhere;
  - `//!` headers past the module's contract and its one non-obvious why;
  - multi-paragraph rationale where one or two sentences carry it.
- **Where it lands:** about 0.25 for each corpus, tree-wide.
  - Rust: about 17,940 comment lines, so about 12,400 cut.
  - Slint: about 5,810, so about 3,400 cut.
- **No code changes.** Every commit a slice ends in is comment-only, so each diff reads as prose.

## Decisions settled here

1. **A slice's commit is comment-only, and a mechanical check says so** (Verification).
   - **What the check does.** It prints every changed line that isn't a comment or blank. A trailing comment on a code line shows up there and is read by eye.
   - **When a comment needs code to shrink.** If it can only get shorter through a rename or a split, that is its own `refactor(...)` commit beside the slice, never inside it. Anything bigger than a small, safe rename goes to Open questions.
2. **Which copy stays follows `CLAUDE.md`'s tiering.**
   - An argument lives at its anchor: the doc comment on the constant, function or module it constrains.
   - A rule keeps only what no single anchor can hold: a coupling between trees, or a comparison across peers. A doc comment re-arguing one keeps its own half and names the rule.
   - `CLAUDE.md` keeps the prohibitions violable from outside the anchor's directory, each with its "argued at" pointer.
   - An ADR is never rewritten. A comment re-arguing an ADR's "why not the alternative" keeps its contract and names the ADR.
   - Where a rule restates a doc comment, the rule's copy is the one to delete (`CLAUDE.md`'s own words). It goes in the commit of the slice it duplicated, so one diff shows the argument kept once. It doesn't move the ratio.
3. **The target is tree-wide, not per crate.** A slice made mostly of `SAFETY:` invariants and platform quirks can end above 0.25 if another ends below. Progress records every slice's before and after, so the running total stays visible.
4. **A slice already under 0.25 still gets the pass**, for register rather than a quota: changelog, narration and second copies go wherever they are.
5. **Tests and the testkit are out, with one exception.** A test comment that cites a production comment this pass changes is updated in the same commit, so the citation stays true. A grep for "doc comment" and "module doc" found six; each is listed under its slice.
6. **`migrations/` is out.** sqlx checksums a shipped migration's whole file, comments included.
7. **The full gate runs on every slice, clippy included.**
   - Clippy reads `///` and `//!`. `doc_markdown`, `doc_lazy_continuation`, `doc_overindented_list_items` and `empty_line_after_doc_comments` all fire on a prose edit.
   - A few tests read comment text, so `cargo test` runs after every slice, not just at the end.
8. **Worst ratio first, crate by crate.** That sets the bar on the small crates before the ones with the most to cut. A crate more than about 800 lines over splits along its own directories, one commit per slice, and its slices run worst first too.

## Structure (who owns what afterwards)

| Where | Owns |
|---|---|
| `///` and `//!` at the anchor | The contract, plus its one non-obvious why. A `//!` header is the module's contract, not its history |
| `//` inside a body | Ordering, lock order, a platform quirk, a magic constant, a workaround with its issue link. Nothing the code already says |
| `// SAFETY:` | The invariant the `unsafe` block relies on (`.claude/rules/unsafe-rust.md`), shortened but whole |
| `.slint` notes | Why a binding is shaped that way (`code-style.md`'s Slint row). `// Translators:` lines are catalog input and stay unchanged |
| `.claude/rules/*.md` | Couplings between trees and comparisons across peers, never a restated doc comment |
| Root `CLAUDE.md` | Prohibitions violable from outside an anchor's directory, each with its "argued at" pointer |
| `docs/adr/` | Why this and not the alternative. Not edited here |
| `.claude/rules/code-style.md` | Gains the target and its yardstick (Phase 14). It is the only place the figure is written |

## Every slice

- [ ] Re-read `.claude/rules/code-style.md`.
- [ ] Measure the slice (Verification) and compare it with Progress. Rescope if it moved.
- [ ] Find what points in. Grep `CLAUDE.md`, `.claude/rules/`, `docs/adr/`, `README.md` and the slice's own tests for its paths and item names. Those comments are condensed, never removed, and each pointer is checked true afterwards.
- [ ] Cut, file by file, starting with the files listed under the phase.
- [ ] The comment-only check prints nothing, or only trailing comments read by eye.
- [ ] Gate: `cargo fmt --all --check`, `cargo clippy --all-targets --locked --workspace -- -D warnings`, `cargo test --locked --workspace`.
- [ ] Measure again and fill in Progress.
- [ ] Commit once Kenan grants the phase: `docs(<crate>): ...`, the crate named without its `melodia-` prefix (`docs(binary)` for `melodia`, `docs(slint)` for the Slint tree).

## Progress

"Over" is how many comment lines the slice holds above 0.25 of its own code. Rust slices are measured with the Rust command and Slint slices with the Slint one.

| Slice | Covers | Ratio before | Comment lines before | Over | After | Commit |
|---|---|---|---|---|---|---|
| 1 | `melodia-artwork` | 0.73 | 1,028 | ~674 | 0.31 (441) | `a6e10d5f` |
| 2 | `melodia-net` | 0.59 | 1,118 | ~648 | 0.24 (448) | `bda588b8` |
| 3 | `melodia-platform` | 0.51 | 1,030 | ~523 | 0.26 (528) | `d58d95b2` |
| 4 | `melodia-audio` | 0.48 | 1,414 | ~677 | 0.25 (729) | `fa210378` |
| 5 | `melodia-engine` | 0.48 | 1,572 | ~756 | 0.23 (746) | `c6dc0322` |
| 6.1 | `melodia-core`: `utils/`, `lib.rs`, `config.rs`, `error.rs` | 0.79 | 561 | ~384 | 0.31 (228) | `670f9ad5` |
| 6.2 | `melodia-core`: `entities/`, `themes/` | 0.39 | 1,094 | ~394 | 0.22 (623) | `a7787e38` |
| 7.1 | `melodia-playback`: the files directly under `player/playback/`, plus `lib.rs` | 0.54 | 1,075 | ~574 | 0.25 (496) | `1ba22f47` |
| 7.2 | `melodia-playback`: `output/` | 0.41 | 1,662 | ~647 | 0.27 (1,084) | `3400c452` |
| 8.1 | `melodia-app`: `library/radio/`, `library/lyrics/` | 0.64 | 912 | ~555 | 0.29 (416) | `a5383164` |
| 8.2 | `melodia-app`: `state/`, `lib.rs` | 0.54 | 282 | ~153 | 0.31 (161) | |
| 8.3 | `melodia-app`: `services/` | 0.44 | 1,233 | ~539 | | |
| 8.4 | `melodia-app`: `tasks/` | 0.42 | 939 | ~377 | | |
| 8.5 | `melodia-app`: the rest of `library/` | 0.40 | 1,712 | ~636 | | |
| 9 | `melodia` (the binary) | 0.42 | 533 | ~216 | | |
| 10.1 | `melodia-views`: the artwork and backdrop tiers | 0.73 | 1,009 | ~665 | | |
| 10.2 | `melodia-views`: `ui/radio/` | 0.51 | 1,607 | ~817 | | |
| 10.3 | `melodia-views`: the tabbed pages, Browse and Search | 0.44 | 1,783 | ~773 | | |
| 10.4 | `melodia-views`: the shell and the player | 0.42 | 1,755 | ~704 | | |
| 10.5 | `melodia-views`: the rest of the shared layer | 0.33 | 2,334 | ~591 | | |
| 10.6 | `melodia-views`: details, lists and settings | 0.29 | 2,284 | ~289 | | |
| 11 | `melodia-store` | 0.34 | 2,456 | ~666 | | |
| 12 | `melodia-integrations` | 0.30 | 936 | ~147 | | |
| 13.1 | Slint `globals/`: playback, shell and radio | 1.15 | 892 | ~698 | | |
| 13.2 | Slint `globals/`: library and dialogs | 0.63 | 682 | ~410 | | |
| 13.3 | Slint root files and `layout/`, plus `melodia-ui`'s Rust | 0.48 | 1,024 | ~489 | | |
| 13.4 | Slint `components/`, its own files | 0.47 | 1,538 | ~725 | | |
| 13.5 | Slint `components/`'s subdirectories | 0.34 | 3,013 | ~788 | | |
| 13.6 | Slint `views/` | 0.29 | 2,055 | ~286 | | |
| **Rust** | production Rust, tree-wide | **0.42** | **30,355** | **~12,400** | | |
| **Slint** | the Slint tree | **0.40** | **9,204** | **~3,400** | | |

## Phases

### Phase 1: `melodia-artwork`
Status: done.

- [x] **1. The crate**, one commit (10 files).
  - **Furthest over:** `media/image/cover_thumbs.rs` (153), `image_decode.rs` (136), `material_you.rs` (102), `artwork/mod.rs` (101).
  - **Pointed at:** `CLAUDE.md`'s `media/` bullet names `image_decode`'s primitives, `material_you`'s resampler, the artwork store, `cover_thumbs`' tiers and `logo_tile`'s floor. `crates/melodia/tests/bounded_decode.rs` walks for the decode exceptions.

### Phase 2: `melodia-net`
Status: done.

- [x] **2. The crate**, one commit (17 files).
  - **Furthest over:** `lyrics_directory/lrclib.rs` (129), `lyrics_directory/recording.rs` (84), `radio_browser/mod.rs` (61), `net/mod.rs` (49).
  - **Pointed at:** `CLAUDE.md`'s `services/` bullet (`http_url`, `read_capped`, `get_capped`, the `pacer`), held by `net_primitives.rs`, `radio_facade.rs` and `lyrics_switch.rs`; `.claude/rules/radio.md`.
  - **Test comment citing it:** `crates/melodia/tests/radio_facade.rs` quotes `radio_browser/mod.rs`'s module doc ("Nothing outside `library::radio` should reach here").

### Phase 3: `melodia-platform`
Status: done.

- [x] **3. The crate**, one commit (25 files).
  - **Furthest over:** `single_instance.rs` (85), `logging.rs` (71), `crash_report.rs` (55), `desktop_integration.rs` (42).
  - **Pointed at:** `CLAUDE.md` on `single_instance::name_is_taken`, `crash_report::prune`, `allocator`, `install_kind` and `dwm_titlebar::is_dark_from_rgb`; `.claude/rules/diagnostics.md` and `desktop-shell.md`.
  - **Test comment citing it:** `system_theme_tests.rs` ("the clamp its doc comment claims").

### Phase 4: `melodia-audio`
Status: done.

- [x] **4. The crate**, one commit (17 files).
  - **Furthest over:** `source/stream_source.rs` (107), `file_decode.rs` (94), `decode.rs` (71), `audio.rs` (68).
  - **Pointed at:** `CLAUDE.md` on `source::decode` and `source/audio.rs`'s vocabulary; `.claude/rules/audio-stack.md` and `radio.md` (the stream path's ring).

### Phase 5: `melodia-engine`
Status: done.

- [x] **5. The crate**, one commit (23 files).
  - **Furthest over:** `engine/backend/mod.rs` (211), `state/transport.rs` (101), `handlers.rs` (74), `backend/output.rs` (73).
  - **Pointed at:** `CLAUDE.md` on `with_state_emit`, `emit_and_execute` and the lock order; `.claude/rules/audio-stack.md`.

### Phase 6: `melodia-core`
Status: done.

- [x] **6.1 `utils/` and the three root files** (`lib.rs`, `config.rs`, `error.rs`), one commit.
  - **Furthest over:** `utils/self_writes.rs` (57), `utils/atomic_file.rs` (53).
  - **Pointed at:** `CLAUDE.md`'s `utils/` bullet (`fold`, `needle`, the three `event_bridge` producers, `exe`, `redact`), `config::Paths::data_root`, and the `AppError` conventions.
- [x] **6.2 `entities/` and `themes/`**, one commit.
  - **Furthest over:** `entities/radio.rs` (173), `entities/lyrics.rs` (89).
  - **Pointed at:** `CLAUDE.md`'s `themes/` bullet (`Palette`'s three semantic slots, `material3.rs` being generated).

### Phase 7: `melodia-playback`
Status: done.

- [x] **7.1 The DSP chain**: the files directly under `player/playback/`, plus `lib.rs`. One commit.
  - **Furthest over:** `spectrum.rs` (136), `equalizer.rs` (85).
  - **Pointed at:** `CLAUDE.md` on `stream_health.rs` and `dsp.rs`; `.claude/rules/audio-stack.md` and `visualizer.md`.
- [x] **7.2 `output/`**, one commit.
  - **Furthest over:** `output/voice.rs` (156), `output/device.rs` (102).
  - **Pointed at:** `CLAUDE.md` on `output::device::open` and `AudioOutput`; `.claude/rules/audio-stack.md`.

### Phase 8: `melodia-app`
Status: in progress, 8.3 next.

- [x] **8.1 `library/radio/` and `library/lyrics/`**, the two doors, one commit.
  - **Furthest over:** `library/radio/logos.rs` (113), `library/lyrics/mod.rs` (91).
  - **Pointed at:** `CLAUDE.md`'s `library/` bullet; `.claude/rules/radio.md`; ADRs 12 and 15.
- [x] **8.2 `state/` and `lib.rs`**, one commit.
  - **Pointed at:** `CLAUDE.md` on `Signal` and `SharedFlag` (`state/signal.rs`).
- [ ] **8.3 `services/`**, one commit.
  - **Pointed at:** `CLAUDE.md` on `view_state::ViewStateData`, `first_launch_theme` and the updater; `.claude/rules/updater.md`.
  - **Test comment citing it:** `support_tests.rs` (`SupportFlags`' doc).
- [ ] **8.4 `tasks/`**, one commit.
  - **Pointed at:** `CLAUDE.md`'s `tasks/` bullet (`one_shot`, `spawner`, `rss_sampler`, `audio_health`).
  - **Test comment citing it:** `radio_logo_cache_tests.rs` ("the module doc argues it in prose").
- [ ] **8.5 The rest of `library/`**, one commit.
  - **Furthest over:** `library/tags.rs` (94), `library/playback.rs` (92).
  - **Pointed at:** `CLAUDE.md` on `PlaybackContext` and `reveal_in_file_manager`; `.claude/rules/library-data.md`.
  - **Test comments citing it:** `tags_tests.rs` ("the module doc's warning"), `support_tests.rs` (`library/settings/support.rs`' two docs), `view_tests.rs` (`snap_to_preset`'s doc).

### Phase 9: `melodia` (the binary)
Status: not started.

- [ ] **9. The crate**, one commit (11 files).
  - **Furthest over:** `main.rs` (57), `boot/ui_setup/views.rs` (54), `shutdown.rs` (39).
  - **Pointed at:** `CLAUDE.md` on the `--version` and `--logs` branches, the order in `main()` and `respawn_if_requested`. `main_order_tests` reads `main`'s body.

### Phase 10: `melodia-views`
Status: not started.

- [ ] **10.1 The artwork and backdrop tiers**, one commit: `backdrop.rs`, `grid_prewarm.rs`, `hero_backdrop.rs`, `hero_chips.rs`, `hero_folds.rs`, `artwork_cache.rs`, `detail_artwork.rs`, `now_playing_artwork.rs`, `mosaic_hero.rs`, `cover_generation.rs`, `name_palette.rs`, and `aurora/`.
  - **Furthest over:** `backdrop.rs` (175), `grid_prewarm.rs` (147).
  - **Pointed at:** `CLAUDE.md` on `blur_spec`, `grid_prewarm::cover_size` and the two artwork newtypes; `.claude/rules/ui-patterns.md`'s Covers block.
- [ ] **10.2 `ui/radio/`**, one commit.
  - **Furthest over:** `radio/detail.rs` (136).
  - **Pointed at:** `CLAUDE.md` on Radio's leave (argued in `ui/radio/mod.rs`) and `detail::reseat`; `.claude/rules/radio.md`.
- [ ] **10.3 The tabbed pages, Browse and Search**: `my_library/`, `favorites/`, `recently_played/`, `search/`, `browse/`. One commit.
  - **Pointed at:** `.claude/rules/ui-patterns.md`'s three-tabbed-pages contract.
- [ ] **10.4 The shell and the player**: `window_chrome/`, `shell/`, `now_playing/`, `visualizer/`. One commit.
  - **Furthest over:** `window_chrome/geometry.rs` (86).
  - **Pointed at:** `CLAUDE.md` on the bridge, `file_drop` and `respawn_target`; `.claude/rules/desktop-shell.md` and `visualizer.md`.
- [ ] **10.5 The rest of the shared layer**: the other 32 files directly under `ui/`, plus `callbacks/` and `paged_rows/`. One commit.
  - **Pointed at:** `CLAUDE.md` on `file_dialog::parented`, `launcher.rs`, `index_persist`, `CALLBACK_HOMES` and `PagedRows`.
- [ ] **10.6 Details, lists and settings**: `albums/`, `artists/`, `genres/`, `playlists/`, `track_detail/`, `tracks/`, `queue_panel/`, `settings/`, `appearance/`, `onboarding/`, `lib.rs`. One commit. Already near the target, so mostly the register pass (decision 4).
  - **Pointed at:** `CLAUDE.md` on `theme_paint` and the system theme watcher; `.claude/rules/ui-patterns.md`'s Settings page.

### Phase 11: `melodia-store`
Status: not started.

- [ ] **11. The crate**, one commit (51 files).
  - **Furthest over:** `media/ingest/metadata.rs` (108), `tag_writer.rs` (99), `database/mod.rs` (71), `queries/radio.rs` (54).
  - **Pointed at:** `CLAUDE.md` on `apply_watch_batch`, `backup.rs` and the write transaction rule; `.claude/rules/library-data.md`, `blake3.md` and `sqlx.md`.

### Phase 12: `melodia-integrations`
Status: not started.

- [ ] **12. The crate**, one commit (24 files).
  - **Furthest over:** `scrobble/mod.rs` (34), `discord/model.rs` (30).
  - **Pointed at:** `CLAUDE.md`'s zbus footgun.

### Phase 13: the Slint tree
Status: not started.

The 36 `// Translators:` notes stay byte for byte: `crates/melodia/tests/translations.rs` holds each equal to its entry in the `.pot`.

- [ ] **13.1 `globals/`, playback, shell and radio**, one commit: `radio`, `player`, `shell`, `lyrics`, `queue`, `audio`, `signal-path`, `hero-backdrop`, `hero-chips`, `drag`, `nav`, `settings-page`, `onboarding`, `updater`, `notifications`, `list-travel`.
  - **Furthest over:** `radio.slint` (129), `player.slint` (116), `shell.slint` (114), `lyrics.slint` (86).
  - **Pointed at:** `globals/nav.slint` as the nav map's authority, and `SettingsPage.category-count`.
- [ ] **13.2 `globals/`, library and dialogs**, one commit: the other 16 files.
  - **Furthest over:** `dialog.slint` (84), `dialog-forms.slint` (55), `curated.slint` (54).
  - **Pointed at:** `CLAUDE.md` on the two dispatchers (`dialog.slint`, `selection.slint`).
- [ ] **13.3 The root files and `layout/`**, one commit: `app-window.slint`, `models.slint`, `theme.slint`, `settings.slint`, the four `layout/` files, and `melodia-ui`'s own Rust (`build.rs`, `src/`: 26 comment lines over 21 of code, counted in the Rust corpus).
  - **Furthest over:** `app-window.slint` (127), `models.slint` (122), `theme.slint` (102), `settings.slint` (94).
  - **Pointed at:** `CLAUDE.md`'s UI Structure (the mount sheet, the `export { }` block, `models.slint` mirroring Rust, `Theme.is-light` and `ink-on`, `resize-ring`).
- [ ] **13.4 `components/`, its own files**, one commit (63 files).
- [ ] **13.5 `components/`'s subdirectories**, one commit.
  - **Furthest over:** `now-playing/` (266), `hero/` (201), `grid/` (170), `track-list/` (104).
  - **Pointed at:** `CLAUDE.md` on `components/onboarding/` (no `changed` handler) and `section-active-gate`; `.claude/rules/ui-patterns.md` and `slint-pitfalls.md`.
- [ ] **13.6 `views/`**, one commit.
  - **Furthest over:** the files directly under `views/` (315 between them), `views/radio/` (132).

### Phase 14: Measure, and hold the target
Status: not started.

- [ ] Measure both corpora and fill in Progress's totals.
- [ ] Write the target into `.claude/rules/code-style.md`'s Comments section: about 0.25 comment lines per line of code, tree-wide, and the two `cloc` commands as the yardstick. `CLAUDE.md`'s code-style bullet keeps pointing at the rule without the figure, so it is written once.
- [ ] Offer the `CLAUDE.md` and `README.md` edits this leaves, if any.
- [ ] Delete this plan, then open the PR.

## Cross-cutting

- **One PR, after Phase 14.** The branch is linked to #125 from the issue's Development sidebar, so the first PR from it that merges closes the issue. Landing in parts would close it early.
- **Keeping up with `main`.** The pass touches nearly every file, so bring `main` in before each phase and measure after.
- **No hand pass, no release build and no RSS run.** Nothing a user sees changes. The gate is the whole check.
- **File size.** Comment-only commits only shrink files, so the 800-line cap can't be crossed.

## Open questions

1. **A pin on the ratio.** A walk in `crates/melodia/tests/` could fail any crate above a ceiling, which would hold the target where a rule can't. Lean no: it would fail an unrelated PR for one well-argued comment, and a ceiling loose enough to avoid that is too loose to catch drift. New tests wait for Kenan's go either way.

## Verification

- **The yardstick** (#125), comment lines over code lines:
  ```bash
  # production Rust
  cloc --include-lang=Rust --exclude-dir=tests,target,melodia-testkit,benches --not-match-f='_tests\.rs$' crates
  # the Slint tree
  cloc --force-lang=C++,slint --include-ext=slint crates/melodia-ui/ui
  ```
  A slice is measured the same way with its paths in place of `crates` or `crates/melodia-ui/ui`.
- **Comment-only check**, before each commit. It prints every changed line that is neither a comment nor blank:
  ```bash
  git diff HEAD -U0 -- '*.rs' '*.slint' \
    | grep -E '^[-+]' | grep -vE '^(\+\+\+|---) ' | cut -c2- \
    | grep -vE '^[[:space:]]*(//|$)'
  ```
  Checked against history: it prints nothing for `259c48d0` and `26eccdd9`, and catches the three code lines `2c15ab98` carried beside its prose.
- **Gate, every slice:**
  - `cargo fmt --all --check`;
  - `cargo clippy --all-targets --locked --workspace -- -D warnings`;
  - `cargo test --locked --workspace`.
