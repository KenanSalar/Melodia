# CLAUDE.md

Melodia, a cross-platform desktop music player: a Slint 1.18 UI over a pure-Rust backend, joined by
direct calls and tokio channels. No WebView, and the only IPC is the single-instance socket.

## Where things go

Rules load only when Claude *reads* a file their `paths:` glob matches, never on a grep or clippy
hit, so this table finds them. A rule missed where needed gets a wider glob, never a copy here.

| file | covers |
|------|--------|
| `.claude/rules/library-data.md` | the artwork ledger and its undo hold, the audio-extension predicate, query shape and the folded search columns, smart-playlist membership |
| `.claude/rules/ui-patterns.md` | the component catalogue, the UI contracts no one file holds, where to reach for grids, lists, filtering and covers, the three tabbed pages, the Settings page |
| `.claude/rules/desktop-shell.md` | window chrome + winit, caption styles, restart-gated settings, the unfocused tint, tray, the volume conversion |
| `.claude/rules/updater.md` | the polkit helper's four spellings, the three platform-key tables |
| `.claude/rules/radio.md` | a new way onto nav 10, stream URLs in logs, the `id == 0` sentinel |
| `.claude/rules/ci-packaging.md` | action pinning, the shared composites, adding a workspace member |
| `.claude/rules/testing.md` | testing practice, not this tree's inventory: the ISTQB principles, how to pick cases, what a corpus walk owes, the Rust attribute and determinism rules, assertion discipline |
| `.claude/rules/*.md` (rest) | per-crate best practices (tokio, sqlx, slint, symphonia, lofty, rayon, serde, blake3, rust-performance) plus `slint-pitfalls`, `unsafe-rust` and `code-style` |

- **A fact goes at its anchor**, the doc comment on what it constrains. A coupling across trees
  (Rust and `.slint`, Rust and CI) goes in a rule, a prohibition violable from anywhere here, and
  why-not-the-obvious-alternative in `docs/adr/`, starting at its `README.md`.
- **Module `//!` docs are the most current tier**: link, don't restate. Two copies of one argument
  is the failure mode, and a rule restating a doc comment holds the copy to delete.
- **A code comment never cites a rule or this file.** Every comment, doc comment and `.slint` note
  states its own reason; the pointing runs the other way, a rule naming the code.
- **Re-read `.claude/rules/code-style.md` before writing comments, and audit each one a change adds
  against its "Auditing a change" before reporting the work done**: only this file survives
  `/compact`. Explain why, never what; be terse; never narrate or write a changelog.
- **A shipped plan doc owes no ADR by itself**: what it built is argued at its anchors before it is
  deleted, and the deletion is the last check that no significant decision went unrecorded.

## Commands

```bash
cargo run -p melodia                       # debug
cargo build --release -p melodia && target/release/Melodia
cargo fmt --all                            # CI gates it with --check
cargo clippy --all-targets --locked --workspace -- -D warnings   # the whole gate; never cargo check
cargo test --locked --workspace            # no doctests: `[lib] doctest = false`
cargo llvm-cov --workspace --html          # coverage → target/llvm-cov/html/
/usr/bin/time -v target/release/Melodia    # peak RSS (release only)
```

`rust-toolchain.toml` argues its pin and what a bump moves; `README.md` lists the system packages.

## Prohibitions violable from anywhere

### Crates and layering

- **No crate re-exports another member's items**, generated Slint types included
  (`melodia_ui::AppWindow`). Every member carries `[lints] workspace = true` and sits directly
  under `crates/`. `workspace_shape.rs` holds all three; the ADRs carry why.
- **The UI reaches the database only through `melodia-app`'s `library/`**: `melodia-views` names
  neither `melodia-store` nor `melodia-net` in its manifest. A library API returning a type views
  can't name means an entity missing from `melodia-core`, never a re-export.
- **A write transaction opens only in `melodia-store`**, each unit of work a store function taking
  plain data, every query that takes a transaction `pub(crate)`.
- **`tasks/` names no `ui::*` and no Slint**, its manifest holding both; a task that needs the UI
  takes it from its caller. `melodia-platform` names `melodia-core` and nothing else.
- **An engine or cover cache that raises a toast, counts a play or reports a missing file** goes
  through `melodia-core`'s `utils` bridges (`toast`, `play_counts`, `missing_artwork`).
- **No Rust or `.slint` file under `crates/` runs past 800 lines** (`tests/` excepted): split by
  cohesion where logic dominates, compress comments where prose does; a plan counts each grown file.

### Facades and primitives

- **Nothing outside `library::radio` names `radio_browser`** (`radio_facade.rs`), and nothing
  outside `library::lyrics` names `lyrics_directory` (`lyrics_switch.rs`).
- **Every fetch parses its URL with `http_url` and reads its body through `read_capped`**, never
  `bytes()` or `.json::<T>()`, the updater's download to a file excepted. `net_primitives.rs`
  walks the scheme test and the streamed read; `bytes()` and `.json()` are review's,
  `str::bytes()` and `RequestBuilder::json` sharing their spellings.
- **An image decodes, resizes and encodes through `image_decode`'s primitives**; `resampler.rs`
  and `bounded_decode.rs` hold the two exceptions.
- **Only `services::platform::allocator` names `libc`.**
- **`open::that_detached`, never `open::that`**, which waits on the launcher, on Linux for the
  browser's whole lifetime (`launcher.rs`).
- **Native dialogs go through `ui::file_dialog::parented`**, the only place `rfd` is named
  (`file_dialog.rs`).
- **`utils::exe::current_exe`, never `std::env::current_exe()`**, for a path you execute, install
  to or write down, Linux appending `" (deleted)"` once an upgrade unlinks it (`binary_path.rs`).

### Errors and logging

- **No `unwrap()` anywhere, tests included** (`unwrap_used = "deny"`), and `expect()` only with
  the invariant in its message. A test reaches for `?`, `assert_eq!`, `matches!` or `let … else`.
- **No `unsafe` outside platform FFI, and never for performance** (`unsafe_code = "deny"`;
  `.claude/rules/unsafe-rust.md` lists the sites).
- **`#[expect]`, never `allow`**, for a genuinely needed suppression: it fires once it goes stale.
- **A `log::` call handed an error formats it with `error::describe`**, never a bare `{e}`,
  whatever the error's type (`logged_errors.rs`). Failure text the user reads takes it too.
- **An `AppError` keeps its cause**: `x(msg, source)`, never `x_msg(format!("…{e}"))`; no error
  carried as a `String`; failures told apart by variant, never by text (`error_as_string.rs`).
- **Never log a credential, token or session key**: users attach the log tail to public issues.

### State, threads and process

- **Lock discipline**: pair `with_state_emit` and `execute_actions` only through
  `emit_and_execute`, whose `exec_lock` spans both and never crosses `.await`. The order is
  `exec_lock → PlayerState → decks → output`, never reversed.
- **A `Signal` moves only through `bump()`**, its subscribers taking `ui::signal::on_signal`, and a
  `SharedFlag`'s mirror is set before its persist is spawned.
- **Kick after persist**: a `mutate_settings` consumer kicks inside the same `spawn_blocking`, after
  the write and only on `Ok(())`; a callback with several writes kicks only if all committed.
- **Two callbacks writing one field share a synchronous shadow**, set before the disk write is
  spawned, and a `views.json` index goes through `IndexPersist` (`index_persist.rs`).
- **A once-per-install sweep is `one_shot::spawn`**, never a hand-rolled copy of its shape.
- **A dev build never shares the installed data root** (`config::Paths::data_root`). Prefs go in
  `settings.json`, per-view state in `views.json`, and a map in any JSON state file is a `BTreeMap`.
- **Migrations run after a backup, and a failed backup is fatal.** A shipped migration is never
  edited: sqlx checksums the file, so every database that applied it fails the next run.
- **A moved file keeps its row through `queries::scan::apply_watch_batch`.** A test never proves
  that by the id, which SQLite re-uses when the deleted row held the largest.
- **The Slint event loop owns the main thread**: never block it, never `ui.set_*` from another
  (`slint::spawn_local`, `invoke_from_event_loop` or `upgrade_in_event_loop` instead).
- **A thread name fits in 15 bytes**, a rayon pool's prefix plus its widest index included, or
  Linux truncates it silently (`packaging.rs`).
- **`--version` and `--logs` are literal-first branches in `main()`**, ahead of everything that can
  fail. `--version` is a contract older clients' updaters depend on (`cli_contract.rs`).
- **`main()` ends in `std::process::exit`, so no destructor runs at quit**: a resource whose drop
  matters closes explicitly ahead of it (`shutdown.rs`, `main_order_tests.rs`).

## Slint

Three rules share the UI globs: `slint.md` general, `slint-pitfalls.md` what builds and is wrong,
`ui-patterns.md` the component to reach for first. The `.slint` tree is `crates/melodia-ui/ui/`.

- **No `slint::slint!`**: `.slint` files compile through `crates/melodia-ui/build.rs`.
- **Scrollbars only through `OverlayScrollbar`**, both stock scrollbar policies `always-off` on
  every scroller (`scrollbars.rs`).
- **An `animate` that can read a palette brush carries `enabled: !Theme.recoloring`**, a Material
  You change writing a blend every frame (`palette_fade.rs`).
- **`opacity` only on a childless element**: one with children is a texture below 1, 0 included,
  so it fades through a `fade` float in its brushes.
- **Polarity through `Theme.is-light`, ink through `Theme.ink-on`**, never a variant id.
- **`globals/` has no barrel**: import the file you need. `app-window.slint`'s export block is the
  one full list, and an import that only feeds it is not dead.
- **Nothing under `components/onboarding/` carries a `changed` handler** (`onboarding.rs`).
- **Favorites, Recently Played and My Library share one contract**, in `ui-patterns.md`; read it
  before touching any of them. Radio is tabbed and deliberately outside it (`ui/radio/mod.rs`).
- **`globals/nav.slint` holds the nav map.**
- **Icons** are Material Symbols ligatures. A new one owes `scripts/icons.txt`, a re-subset by
  glyph id (`scripts/subset-icon-fonts.py`) and a clean `scripts/check-icons.py`, or it is tofu.
- **Fonts** are Vazirmatn, built by `scripts/patch_vazirmatn.py`. A missing character goes into
  that merge, never a second `import`; a new face owes `licenses/ATTRIBUTION.txt` (`packaging.rs`).

## Internationalization

- **`@tr("English msgid")`**: English is the source, so there is no `en.po`.
- **`@tr` translates literals only**: a `[string]` Rust fills renders untranslated, so the use site
  takes an inline `[@tr("A"), @tr("B")]` in the Rust order instead.
- **Don't translate** ligatures, asset paths, theme tokens, debug logs, brand chips (`"Melodia"`,
  `"KDE"`, `"GNOME"`, `"macOS"`, `"Windows 11"`, Catppuccin names) or `Dialog.confirm-label: "OK"`.
- **A new, changed or removed `@tr` string means `scripts/update-translations.py`**, then finishing
  every catalog: no empty `msgstr`, no `#, fuzzy` (`translations.rs`).
- **A translator note is one `// Translators:` line directly above the `@tr`**, with no second
  `@tr` on that line.
- **A new locale's catalog** starts from `crates/melodia-ui` with
  `msginit --no-translator -l <code> -i translations/melodia-ui.pot -o translations/<code>/LC_MESSAGES/melodia-ui.po`,
  the script merging only into catalogs that exist. Fill its header as the siblings' are, keeping
  the plural rule `msginit` wrote: a sibling's copied in passes `msgfmt -c` and `translations.rs`
  alike. `SUPPORTED_LOCALES` lists the rest it owes.
- **The catalog basename tracks the package name**: slint-build derives the gettext domain from
  `CARGO_PKG_NAME`, so renaming `melodia-ui` renames every catalog.
- **RTL is deferred**, and there is no Rust-callable `tr()`.

## Testing and CI

- **Unit tests live in per-module `tests/` subdirectories**, through
  `#[path = "tests/<name>_tests.rs"]`, never inline. This overrides `testing.md`.
- **A corpus walk lives in `crates/melodia/tests/`, a pin on one named file beside that file**, so
  `cargo test -p <crate>` stays a question about that crate.
- **The suite assumes an LF checkout**, which `.gitattributes` guarantees.
- **Read `.claude/rules/ci-packaging.md` before touching `.github/`.**
- **A release is a pushed `v*.*.*` tag, never a merge.** Land the version bump as a PR, tag the
  merge commit, then QA the draft `release.yml` leaves and publish it by hand.
- **No closing keyword**: link the issue in the Development sidebar, which closes it on merge.

## Memory

- **Memory is a product requirement** (`docs/adr/`): check peak RSS with `/usr/bin/time -v` after
  notable changes, and a feature adding over 20 MB idle RSS gets `heaptrack` before it merges.
- **`RUST_LOG=info MELODIA_RSS_SAMPLE=1`** logs `VmRSS`, `RssAnon` and `RssFile` tagged with the
  current view. `RssFile` growth is the Mesa GPU pool, not the heap.
- **The glibc arena cap is set before the logger and the runtime start a thread**, in
  `prepare_process`. Don't lower it to 1.

## Known Gaps

- **zbus**: never enable its `tokio` feature, which panics at startup (`Cargo.toml` argues it), and
  `ksni` stays off its default `tokio` feature for the same reason.
- **Vendored winit fork**: `winit/`, wired by `[patch.crates-io]`, with `dpi` deliberately not
  vendored. `winit/README.md` says what it adds and what retires it.

## User Conventions

- Always call big files/objects/functions **'monolithic'** — no synonyms.
