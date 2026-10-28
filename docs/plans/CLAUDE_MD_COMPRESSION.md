# Plan: Compress the root CLAUDE.md, and take its citations out of the code

Status: **Phases 1 to 4 done**, Phase 5 next · Owner: Kenan · Created: 2026-10-28 · branch
`docs/trim-comments`

> **Measured 2026-10-28** against `c05b7574`. Every figure below comes from a command run that
> day. Each phase re-greps what it touches before it starts, since the branch moves under it.

Working doc. Delete when every phase has landed (Phase 5).

## Context

**What you see today.**
- **The root `CLAUDE.md` is monolithic: 921 lines and 76,197 bytes.** Before the 2026-10-28 wrap
  it was 211 lines averaging 355 characters each. Anthropic's memory guide targets "under 200
  lines per CLAUDE.md file. Longer files consume more context and reduce adherence." It loads in
  every session and is the one file re-injected after `/compact`, so every line costs on every
  task.
- **Half of it describes what the code already says.** The Module Map (214 lines) and Important
  Conventions (214) are the two largest sections, and an audit of every bullet found most of
  them argued again at their anchors. The best-practices guide lists "describe your architecture
  and file layout in `CLAUDE.md`" among advice to drop, and `/doctor` cuts exactly that.
- **160 bold spans.** The same guide: piled-on emphasis "mostly makes Claude over-apply the rule".
- **Some entries are wrong** (all verified 2026-10-28, listed in Phase 4).
- **21 lines in 17 files give "root `CLAUDE.md`" as their reason** instead of stating it (19
  comments and two test assertion messages), which is the behaviour `CLAUDE.md` already bans for
  `.claude/rules/` citations.
- **Three documents contradict the tree or `CLAUDE.md`**: ADR 1's plan-doc clause, `tokio.md`'s
  mutex advice, `slint.md`'s animation token name.
- **`README.md:374` calls `CLAUDE.md` "the architecture reference"**, which stops being true here.

**Outcome.**
- **`CLAUDE.md` is under 200 lines at 100 columns**, about 180, holding only:
  - the commands that confirm work;
  - the index of the rules and the criterion for where a fact goes;
  - prohibitions violable from anywhere, each naming the lint or test that holds it;
  - gotchas Claude would otherwise rediscover, and workflow decisions not in the code;
  - **User Conventions, kept verbatim as its last section** (Decision 5).
- **No comment, config comment or test message cites root `CLAUDE.md`.** Each states its reason.
- **Content with no other home is moved to its anchor first**, never cut.
- **ADR 1 is corrected in place**, `tokio.md` and `slint.md` match the tree, `README.md:374`
  describes `CLAUDE.md` as what it now is.
- **What keeps working:**
  - every rule citing root `CLAUDE.md` still lands on a recognisable lead;
  - every ADR reference to `CLAUDE.md` still holds;
  - no rule's frontmatter changes, so every rule loads on the same files;
  - no behaviour changes: the code diff is comment lines only, plus two test assertion messages.

## Decisions settled here

1. **What stays in `CLAUDE.md`** is what Claude would get wrong without it and can't learn from
   the anchor. Per bullet, cut:
   - architecture overviews, module and file inventories, directory listings;
   - anything already argued at its anchor (the second copy `CLAUDE.md` itself warns against);
   - anything a path-scoped rule holds (it loads where the fact matters);
   - history ("used to", "shipped broken", "was exactly that until");
   - anything the compiler or clippy states plainly (`gen` is reserved).

   A prohibition violable from anywhere stays, in one or two lines, with the lint or test that
   holds it. Wrap at 100 columns, no em dashes in new prose.
2. **No comment cites root `CLAUDE.md`** (Kenan, 2026-10-28). The ban on citing a rule from code
   extends to `CLAUDE.md`, for the same reason: a comment that points elsewhere for its reason
   breaks the moment the target is compressed. It covers comments in Rust, Slint, scripts,
   `*.toml`, `.gitignore` and workflows, module and test docs, and test assertion messages.
   - **Rules may still cite root `CLAUDE.md`**: it always loads, and the rules are Claude's
     tooling too. Phase 4 keeps every lead they name.
   - **Docs may name `CLAUDE.md` as a document** (`README.md`, the PR template's checklist).
3. **Move before cut.** A bullet whose argument no anchor carries is moved to its anchor in
   Phase 2, before Phase 4 removes it. A bullet is never cut on the strength of an anchor that
   points back at `CLAUDE.md`.
4. **ADR 1 is fixed directly, with no amendment** (Kenan, 2026-10-28). Its clause that a plan doc
   "reaching the point of deletion now leaves an ADR behind first" was wrong when it was written.
   That makes it a correction, not a decision that changed or a fact that moved since, which is
   what `docs/adr/README.md`'s dated `**Amendment:**` paragraph is for. `CLAUDE.md`'s statement is
   the correct one: a shipped plan doc owes no ADR by itself; what it built is argued at its
   anchors before deletion, and the deletion is the last check that no significant decision
   inside it went unrecorded.
5. **User Conventions stays, verbatim, as the last section** (Kenan, 2026-10-28):
   `Always call big files/objects/functions **'monolithic'** — no synonyms.` No phase rewords,
   moves or cuts it.
6. **Bold marks a bullet's lead and nothing else.**
7. **The gate.** Markdown-only edits need none. Any `.rs` or `.slint` edit, comments included,
   runs `cargo fmt --all --check` and
   `cargo clippy --all-targets --locked --workspace -- -D warnings`. No tests, except
   `cargo test --locked --workspace --test rules_globs` after a rule edit and
   `--test translations` if a `// Translators:` line is touched (none is planned).
8. **Claude never commits or stages.** Each phase ends in its report, and Kenan runs `/ship`.
9. **This file owes no ADR** (Decision 4). Phase 5 deletes it.

## Structure (who owns what afterwards)

| Where | Owns |
|---|---|
| `///`, `//!` and `.slint` notes at the anchor | The contract and its one non-obvious why. Never "see `CLAUDE.md`" |
| `.claude/rules/<subsystem>.md` | Couplings across trees, comparisons across peers. May cite root `CLAUDE.md` |
| Root `CLAUDE.md` | Commands, the rules index and placement criterion, prohibitions violable from anywhere with their tests, gotchas, workflow, User Conventions |
| `docs/adr/` | Why this and not the obvious alternative. ADR 1 corrected here, nothing else edited |
| `README.md`, `CONTRIBUTING.md` | People. Prerequisites live in `README.md`, not `CLAUDE.md` |
| This file | The remaining work. Deleted in Phase 5 |

## Progress

| Phase | What | Before | After |
|---|---|---|---|
| 1 | Fix the contradictions outside `CLAUDE.md` | 4 files wrong | done, plus 2 stale lines found on the way |
| 2 | Move what has no other home | 7 moves | done, and 3 of Phase 3's pointers dropped in the same edits |
| 3 | Sweep the 21 citations | 21 lines in 17 files | 0 left (3 went in Phase 2) |
| 4 | Rewrite `CLAUDE.md` | 921 lines / 76,197 bytes, 160 bold | 195 lines / 13,466 bytes, 62 bold |
| 5 | Retire the plan | | |

## Phases

### Phase 1: Fix the contradictions outside `CLAUDE.md`
Status: done 2026-10-28. One commit. Markdown and two comments. Also fixed on the way:
`tokio.md`'s first bullet (a sync guard across `.await` is a compile error under `tokio::spawn`,
not a panic) and `walk.rs`'s `spellings_outside` doc ("three walks", five today).

- [x] **ADR 1, corrected in place** (Decision 4). `docs/adr/0001-record-architecture-decisions.md`
  lines 21 to 24: replace "except that one reaching the point of deletion now leaves an ADR behind
  first. That last clause is the whole mechanism, and it is attached to something that already
  happens rather than to a habit somebody has to remember." with the correct rule: before a plan
  doc is deleted, what it built is argued at its anchors, and an ADR is written only where it
  carried a significant decision, the deletion being the last check that none went unrecorded.
  No `**Amendment:**` paragraph, no status change. Re-read the whole ADR afterwards: its Trade
  paragraph mentions deleted working docs and must still agree. An ADR names no plan file.
- [x] **`tokio.md`'s mutex bullet** says "Prefer `std::sync::Mutex` over `tokio::sync::Mutex`",
  where `CLAUDE.md` says `parking_lot::Mutex` and the tree has 70 `parking_lot` uses against 21
  `std`. Before rewording, read the `std` sites: `melodia-store` and `melodia-testkit` carry no
  `parking_lot`, and `PlayerState` and `exec_lock` in `melodia-engine` use `std` on purpose. State
  the rule the tree follows (a synchronous mutex unless the lock crosses `.await`,
  `parking_lot::Mutex` where the crate carries it), so both files say one thing.
- [x] **`slint.md`'s Animations bullet** names `Theme.duration-fast` and `Theme.duration-medium`.
  The tokens are `Theme.dur-fast` and `Theme.dur-med` (`theme.slint:185-186`).
- [x] **`crates/melodia-testkit/src/walk.rs:17`** says "ten files stand on that corpus". Count the
  files naming `MIN_SOURCES` and state it without a number if it would rot.
- [x] **`README.md:374`**: "`CLAUDE.md` is the architecture reference" becomes what it is after
  Phase 4: the repo-wide rules and commands for a coding agent, the code's doc comments being
  the architecture reference.

### Phase 2: Move what has no other home
Status: done 2026-10-28. One commit. Comment lines and markdown only. The pointers at
`backup.rs:2`, `redact.rs:6` and `overlay-scrollbar.slint:3` sat in the very lines being rewritten,
so they went here rather than in Phase 3. Rechecked first: the scrollbar slot, the card track,
`TrackListScrollbars`, `CompositeScrollbars` and the track list's missing horizontal bar are all
argued at their own files already, so only the mount steps moved.

Each move lands where the fact is enforced. The audit found these arguments carried nowhere but
`CLAUDE.md`; check each again before moving.

- [x] **`crates/melodia-store/src/database/backup.rs`'s `//!`** gets the three invariants it
  points at `CLAUDE.md` for (line 2): the copy is `VACUUM INTO` through the open connection,
  never a file copy, WAL mode meaning `melodia.db` alone isn't the database; it is staged under
  `.tmp` and renamed, so the final name existing means it is complete; retention deletes only
  names it parses back into its own scheme. Its restore note (lines 4 to 6) already says to
  delete `-wal`/`-shm`; keep one copy.
- [x] **`crates/melodia-core/src/utils/redact.rs:6`** states why `dirs::home_dir()` and never
  `$HOME`: the variable is unset on Windows, whose users have no console to read the paths from,
  while `README.md` and `bug_report.yml` both promise `~`.
- [x] **`crates/melodia-ui/ui/components/overlay-scrollbar.slint`'s header** gets the mount steps
  no file holds: `ScrollView` with both policies `always-off`; the bar at the view root, a
  sibling of the padded layout, pinned by absolute coordinates; the round trip through
  `offset: -sv.content-y` and `scroll-to(o)` (no `<=>`, the sign flip blocks it); one bar per
  axis that can overflow, never gated behind an `if`; inside the `if` branch where the scroller
  is; a gutter rather than a bar over a control. `scrollbars.rs` holds only the policies, the
  dialog track and the hand-rolled `TrackListScrollbars`, so the header must not claim more.
- [x] **`crates/melodia-ui/ui/theme.slint`** at `is-light`: it is the only polarity signal that
  reaches Slint, `Palette` carrying no light/dark flag and the generated palettes no variant id,
  so polarity is measured, never matched.
- [x] **`crates/melodia-ui/ui/app-window.slint:74`**, the re-export block: no barrel, every
  consumer imports the file it needs, and an import here that only feeds the block is not dead.
- [x] **`.claude/rules/testing.md`** gets the five things a corpus walk owes, as practice and
  without the tree's names: a vacuity floor, kept loose; an equality wherever a vanishing subtree
  is the failure; unreadable paths collected and asserted empty, never skipped; comments stripped
  before the needle and brace-matched scope where a substring would lie; an exemption held to an
  exact count, each entry asserted to still match. ADR 36 already says `testing.md` holds these,
  and its globs cover `crates/melodia/tests/` and the testkit. The tree's names (`MIN_SOURCES`,
  `CALLBACK_HOMES`, `scrollbars.rs`) stay at `walk.rs` and the walks. Frontmatter untouched.
- [x] **`docs/adr/README.md`'s "When not to write one"** gets the parts of `CLAUDE.md`'s ADR
  paragraph it lacks: significant means it shapes the structure, a quality the whole app must
  hold, an external dependency, an interface other code builds on or how the code is built;
  write it when the decision is made; a plan doc reaching deletion owes none by itself, the
  deletion being the last check. Not an ADR edit: the README is the collection's guide.

### Phase 3: Sweep the 21 citations
Status: done 2026-10-28. One commit, one Edit per site. Comment lines only, except the two test
messages in `scrollbars.rs`, the one sanctioned non-comment change. Also fixed on the way:
`view_model_strings.rs` named `PlayerViewModel` as what the two strings reach; they reach
`QueueVm` and `PlayerVm`.

Each states its own reason in about the length of the pointer it replaces (Decision 2). Re-grep
first: `git grep -n 'CLAUDE\.md' -- . ':!docs' ':!.claude' ':!CLAUDE.md' ':!README.md'
':!CONTRIBUTING.md' ':!.github/pull_request_template.md'`.

| Site | States instead |
|---|---|
| `.gitignore:81` | heaptrack output from the memory regression checks, no pointer |
| `rust-toolchain.toml:9` | the four things a bump moves: this file, `Cargo.toml`'s `rust-version`, `clippy.toml`'s `msrv`, `README.md`'s prerequisites |
| `crates/melodia-app/src/services/diagnostics.rs:6` | the log tail is safe only while no `log::` call writes a credential, which no walk can check |
| `crates/melodia-core/src/entities/locale.rs:8` | what a new locale owes: a row here and in `LOCALE_NATIVE_NAMES`, a catalog started with `msginit`, a `LANGUAGE_LETTERS` row in `scripts/patch_vazirmatn.py` where the faces lack its alphabet |
| `crates/melodia-core/src/themes/palette.rs:28` | no struct-update fallback, so a palette that forgets a semantic slot fails to compile instead of greying the generated palettes |
| `crates/melodia-core/src/themes/palette.rs:161` | the third copy is pinned by `dwm_titlebar`'s own tests |
| `crates/melodia-core/src/utils/redact.rs:6` | done in Phase 2 |
| `crates/melodia-playback/src/player/playback/output/device.rs:4` | CI's null device depends on it (`crates/melodia/tests/headless.rs`). `rustfmt --edition 2024 --check` this file: it sits in `output/` |
| `crates/melodia-store/src/database/backup.rs:2` | done in Phase 2 |
| `crates/melodia-ui/ui/app-window.slint:1` | `scripts/patch_vazirmatn.py` argues the build |
| `crates/melodia-ui/ui/app-window.slint:140` | Slint's handler for `resize-border-width` hit-tests square corners (`frameless_tests.rs`) |
| `crates/melodia-ui/ui/components/onboarding/onboarding-overlay.slint:12` | the branch drops on close, so a tracker on anything outside it survives the first open and panics on the second (`crates/melodia/tests/onboarding.rs`) |
| `crates/melodia-ui/ui/components/overlay-scrollbar.slint:3` | done in Phase 2 |
| `crates/melodia-ui/ui/globals/onboarding.slint:2` | the same reason as `onboarding-overlay.slint:12`, in a clause |
| `crates/melodia-ui/ui/layout/shortcut-scope.slint:85` | the first-run card mounts below `DialogOverlay`, so a dialog paints over it and takes Escape first |
| `crates/melodia-views/src/ui/onboarding/mod.rs:5` | the same reason as `onboarding-overlay.slint:12` |
| `crates/melodia-views/src/ui/settings/locale.rs:44` | two callbacks read and write `settings.locale`, and a race through disk reads the old value |
| `crates/melodia/tests/scrollbars.rs:7` | the rule itself, as the module doc of the walk that holds it |
| `crates/melodia/tests/scrollbars.rs:155` | message names the reason, not "(CLAUDE.md, Slint Conventions)" |
| `crates/melodia/tests/scrollbars.rs:304` | same |
| `crates/melodia/tests/view_model_strings.rs:9` | the criterion itself: a check holding two trees against each other lives here |

### Phase 4: Rewrite `CLAUDE.md`
Status: done 2026-10-28. One commit. Markdown only. 195 lines rather than about 180: the first
draft ran to 236, so Styling folded into Slint, Testing into CI, and the thread and CLI lines
into State. The arena cap line says "before the logger and the runtime start a thread", the cap
not being the first call in `prepare_process`. `ci-packaging.md:45` now points at
`output::device` and `headless.rs`.

Write it top to bottom in this shape, about 180 lines. Each kept prohibition is one or two lines
and names what holds it. Before writing, re-grep every citation into `CLAUDE.md` (Verification).

- [x] **Intro** (2 lines): what the app is, Slint and pure Rust, no IPC but the single-instance
  socket.
- [x] **Where things go** (about 15):
  - the rules table, kept: a rule loads only when Claude reads a matching file, so the index is
    how Claude learns one exists;
  - the criterion in four lines: a fact goes at its anchor, a coupling across trees in a rule, a
    prohibition violable from anywhere here, why-not-the-alternative in `docs/adr/`;
  - module `//!` docs are the most current tier; link, don't restate;
  - **a code comment never cites a rule or this file** (Decision 2);
  - re-read `code-style.md` before writing comments, it being the rule most often missing after
    a `/compact`;
  - a shipped plan doc owes no ADR by itself (Decision 4);
  - cut: the nested-`CLAUDE.md` tier (none exist), "that test" (no antecedent), the long ADR
    definition (Phase 2 moved it).
- [x] **Commands** (about 10): the current block; the toolchain pin, the four things a bump moves
  being argued at `rust-toolchain.toml` after Phase 3; prerequisites as a pointer to `README.md`,
  whose list is the correct one.
- [x] **Prohibitions violable from anywhere** (about 70), grouped by concern, each naming what
  holds it:
  - **Crates and layering:**
    - no crate re-exports another member's items, every member carries `[lints] workspace = true`
      and sits under `crates/` (`workspace_shape.rs`);
    - `melodia-views` reaches the database only through `melodia-app`'s `library`, and a type it
      can't name means a missing entity in `melodia-core`, never a re-export (the manifests);
    - a write transaction opens only in `melodia-store`;
    - `tasks/` names no `ui::*` and no Slint (the manifest);
    - `melodia-platform` names only `melodia-core`;
    - `utils/`, `toast` and `play_counts` sit low so `player/` names neither `services` nor
      `tasks`.
  - **Facades and primitives:**
    - nothing outside `library::radio` names `radio_browser` (`radio_facade.rs`);
    - nothing outside `library::lyrics` names `lyrics_directory` (`lyrics_switch.rs`);
    - every fetch through `http_url` and `read_capped` (`net_primitives.rs`);
    - the `image_decode` primitives (`resampler.rs`, `bounded_decode.rs`);
    - only `services::platform::allocator` names `libc`;
    - only `open::that_detached` (`launcher.rs`), only `file_dialog::parented` names `rfd`
      (`file_dialog.rs`);
    - `utils::exe::current_exe`, never `std::env::current_exe()` (`binary_path.rs`).
  - **Errors and logging:**
    - no `unwrap` anywhere, tests included (`unwrap_used = deny`), `expect` only with the
      invariant;
    - no `unsafe` outside platform FFI (`unsafe_code = deny`, `unsafe-rust.md`);
    - no `#[allow(dead_code)]`, `#[expect(lint, reason)]` never `allow`;
    - a `log::` call takes `error::describe`, never a bare `{e}` (`logged_errors.rs`);
    - `x(msg, source)` never `x_msg(format!("{e}"))`, no `Result<_, String>`, branch on the
      variant never the text (`error_as_string.rs`);
    - never log a credential.
  - **State and persistence:**
    - pair `with_state_emit` and `execute_actions` only through `emit_and_execute`, lock order
      `exec_lock → PlayerState → decks → output` (`audio-stack.md` cites this lead);
    - `Signal` moves only through `bump()`, `SharedFlag` mirror set before the persist;
    - kick after persist, inside the same `spawn_blocking`, only on `Ok`;
    - sibling callbacks share a synchronous shadow; a `views.json` index goes through
      `IndexPersist` (`index_persist.rs`);
    - a once-per-install sweep is `one_shot::spawn`;
    - a dev build never shares the installed data root; `settings.json` holds prefs and
      `views.json` per-view state; every persisted map is a `BTreeMap`;
    - migrations run after a backup, fatal if it fails, and a shipped migration is never edited;
    - a moved file keeps its row through `apply_watch_batch`, and a test never proves survival
      by the id, which SQLite re-uses.
  - **Threads:** a thread name fits in 15 bytes (`packaging.rs`).
  - **Process and CLI:** `--version` and `--logs` are literal-first branches, `--version` a
    contract older clients' updaters depend on (`cli_contract.rs`; `updater.md` cites this lead).
- [x] **Slint** (about 15):
  - three rules share the UI globs, `slint-pitfalls.md` being what builds and is wrong;
  - no `slint::slint!`;
  - scrollbars only through `OverlayScrollbar`, both policies `always-off` (`scrollbars.rs`);
  - an `animate` that can read a palette brush carries `enabled: !Theme.recoloring`
    (`palette_fade.rs`), `theme_paint` being the brushes' only writer;
  - polarity through `Theme.is-light`/`ink-on`, never a variant id;
  - no barrel, the export block's imports aren't dead;
  - nothing under `components/onboarding/` carries a `changed` handler (`onboarding.rs`);
  - the three tabbed pages share the contract in `ui-patterns.md`, Radio deliberately outside it;
  - `globals/nav.slint` holds the nav map;
  - `ui-patterns.md` cites "Native dialogs go through `ui::file_dialog::parented`" here: keep
    that lead in the prohibitions.
- [x] **Styling** (about 6):
  - a new icon ligature owes `scripts/icons.txt`, a re-subset by glyph id and
    `scripts/check-icons.py`;
  - **Fonts** and **Icons** keep those leads (`slint.md` cites both): coverage goes into the
    Vazirmatn merge, never a second `import`; a face owes an attribution (`packaging.rs`);
  - first-launch theme and system dark/light: cut, argued at `settings` and
    `system_watcher.rs`.
- [x] **i18n** (about 15):
  - `@tr` translates literals only, so a Rust-populated `[string]` takes an inline literal list;
  - don't translate ligatures, asset paths, tokens, debug logs, brand chips, `"OK"`;
  - a changed `@tr` string means `scripts/update-translations.py` and finishing every catalog,
    no fuzzy or empty entry (`translations.rs`);
  - a translator note is one `// Translators:` line directly above the `@tr`, none sharing it;
  - a new locale: the `msginit` recipe, the header filled as the siblings', its plural rule
    from `msginit` and never copied from a sibling;
  - the catalog basename tracks the package name;
  - RTL is deferred;
  - cut: the plural-forms prose per language, which restates the `.po` headers.
- [x] **Testing** (about 5): unit tests in per-module `tests/` subdirectories via `#[path]`,
  never inline (this overrides `testing.md`); a corpus walk lives in `crates/melodia/tests/`, a
  pin on one file beside it; the LF checkout through `.gitattributes`; cut the rest (Phase 2
  moved the walk obligations, `testing.md` holds paths joined).
- [x] **CI and release** (about 6): `ci-packaging.md` before anything under `.github/` or
  `packaging/`; a release is a pushed `v*.*.*` tag, never a merge; never write a closing keyword,
  the sidebar link closes the issue.
- [x] **Memory** (about 6): `/usr/bin/time -v` after notable changes, 20 MB idle RSS means
  heaptrack before merging; `MELODIA_RSS_SAMPLE=1`; the arena cap first in `prepare_process`,
  never lowered to 1.
- [x] **Known gaps** (about 4): the zbus `tokio` feature panics at startup, and `ksni` pinned for
  the same reason; the vendored winit fork, a pointer to `winit/README.md`. Keep the "Known
  Gaps" heading: `desktop-shell.md` cites it.
- [x] **User Conventions**, last and verbatim (Decision 5).

**Must not survive the rewrite** (each verified wrong on 2026-10-28):
- `view_model_tx` and `queue_tx`: now `PlayerSinks::view_model` and `PlayerSinks::queue`, and the
  watches number four, `awaited_start` included.
- `handlers.rs` writing through `queries::track`: it hands a `SnapshotSink`.
- "shared as `Arc<Runtime>`": `AppState::runtime` is a `Handle`.
- `entities/` under `melodia-store`: it is `melodia-core`'s.
- `ContentArea`: no such element.
- `TrackRow`, `PositionTick`, `PlayerViewModel` in `models.slint`: `TrackSummaryRow`,
  `TrackListRow` and `PlayerVm`; `melodia_ui::TrackRow` in the melodia-ui bullet too.
- "the only place [the nav map] is written down": `globals/nav.slint:7-11`.
- "`utils::exe`'s own tests walk the corpus": `crates/melodia/tests/binary_path.rs` does.
- "the citations still in the tree predate this": none remain.
- Prerequisites listing GTK without `libasound2-dev` and `libxkbcommon-dev`.
- The persistence list without `scrobble_credentials.json` and `scrobble_queue.json`.
- "walks the tree for all of this" for `scrollbars.rs`.
- "ten pins" sharing the walk floors.
- `ui/` "and that is all it is": it also holds the view slices.
- "A shipped plan doc owes no ADR" stays; ADR 1 now agrees (Phase 1).

**Reported by the audit, recheck if the line survives:** `error::describe` sits in
`melodia_core::error`, not `utils/`; not every `library` function is `async`; `allocator::trim()`
also runs from about eight view teardowns; the winit retirement names `file_drop` where
`Cargo.toml` and `winit/README.md` name `winit_filter`.

**Citations into `CLAUDE.md` that must still land** (re-grep in Verification):
- `.claude/rules/audio-stack.md:23`, lock discipline;
- `.claude/rules/ci-packaging.md:45`, the headless audio shim: its bullet is cut, so point that
  line at `output/device.rs` and `crates/melodia/tests/headless.rs` in this phase;
- `.claude/rules/desktop-shell.md:105`, Known Gaps;
- `.claude/rules/slint.md:172`, Fonts and Icons;
- `.claude/rules/ui-patterns.md:261`, `file_dialog::parented`;
- `.claude/rules/unsafe-rust.md:60`, "`#[expect]`, never `allow`";
- `.claude/rules/updater.md:24`, `--version`;
- `docs/adr/0001` and `docs/adr/README.md:153`, prohibitions violable from anywhere live here;
- `README.md:374` (Phase 1).

### Phase 5: Retire the plan
Status: not started.

- [ ] Confirm Progress is complete and every phase landed.
- [ ] Check that no decision recorded only here goes unrecorded.
- [ ] Offer any `README.md` or `CONTRIBUTING.md` edits the phases leave.
- [ ] Delete this file. It owes no ADR (Decision 4).

## Open questions

1. **A walk holding Decision 2.** Nothing enforces "no comment cites a rule or `CLAUDE.md`"; a
   corpus walk in `crates/melodia/tests/` could, with the docs and the PR template exempt. A new
   test needs Kenan's go, so it is not a phase.

## Verification

All from the repo root, in zsh.

- **Size and width:**
  ```bash
  wc -lc < CLAUDE.md
  awk 'length($0) > 100 && $0 !~ /^\|/' CLAUDE.md
  command grep -o '\*\*[^*]*\*\*' CLAUDE.md | wc -l
  ```
- **No citation of `CLAUDE.md` left in code** (Phase 3), the docs that may name it excluded:
  ```bash
  git grep -n 'CLAUDE\.md' -- . ':!docs' ':!.claude' ':!CLAUDE.md' ':!README.md' \
    ':!CONTRIBUTING.md' ':!.github/pull_request_template.md'
  ```
- **Every citation into `CLAUDE.md` lands** (Phase 4): print each and find its lead.
  ```bash
  git grep -n -E 'root `?CLAUDE\.md|`CLAUDE\.md`' -- .claude/rules docs/adr README.md
  ```
- **Comment-only diff** (Phases 2 and 3): only the two `scrollbars.rs` messages may print.
  ```bash
  git diff HEAD -U0 -- '*.rs' '*.slint' | command grep -E '^[-+]' \
    | command grep -vE '^(\+\+\+|---) ' | cut -c2- | command grep -vE '^[[:space:]]*(//|$)'
  ```
- **Identifier check** over the new `CLAUDE.md`: every backticked name resolves.
  ```bash
  command grep -o '`[^`]*`' CLAUDE.md | tr -d '`' | command grep -oE '[A-Za-z_][A-Za-z0-9_]{2,}' \
    | sort -u | while read -r w; do git grep -qw -F "$w" -- crates scripts .github docs packaging \
    || echo "missing: $w"; done
  ```
- **User Conventions intact:** `tail -3 CLAUDE.md` shows the section and its one line unchanged.
- **Gates:** `cargo fmt --all --check`; `rustfmt --edition 2024 --check` on `output/device.rs`;
  clippy whenever a `.rs` or `.slint` file changed; `cargo test --locked --workspace --test
  rules_globs` after a rule edit.

## Tools

- **Print the doc comment above an item:** the `docof` helper in `RULE_COMPRESSION.md`'s Tools.
- **Shell notes:** `grep` is ugrep, so script with `command grep` or `git grep`; `ls` is
  colourised; quote globs and `:!` pathspecs in zsh.
