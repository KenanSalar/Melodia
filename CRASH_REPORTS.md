# Crash reports and collectable logs (issue #39)

## Context

Melodia is publicly released with an auto-updater pushing to installs nobody can enumerate,
and **every crash and every log line it produces goes nowhere**. `main.rs:144` is a bare
`env_logger::init()` — stderr only — and there is no `panic::set_hook` in the tree. Launched
from a `.desktop` entry, the tray, or the Windows Start Menu (where
`windows_subsystem = "windows"` means there is no console at all), stderr is discarded. When a
user says "it crashed", there is no artifact to ask them for.

That contradicts a decision already made deliberately: `[profile.release]` sets
`strip = "debuginfo"` rather than `strip = true` *specifically so end-user panic backtraces stay
readable*, and the comment says so. The binary size is being paid and every backtrace thrown away.
`panic = "abort"` sharpens it further — with no unwinding, **every** Rust panic anywhere in the
process is fatal, so a panic hook is not a partial measure but complete coverage of the Rust half.

Issue #39 also names the concrete symptom: `.github/ISSUE_TEMPLATE/bug_report.yml:100` asks
reporters to run `RUST_LOG=info melodia` from a terminal, which nobody running an AppImage or MSI
will do, and which the Windows line already concedes is impossible.

**Outcome:** logs land in a file on every run with no env var set; a panic writes a report with
payload, location, thread, backtrace and environment; Settings → About gains a Diagnostics card
that opens that folder or saves one redacted `.txt` to hand over; and a crash from the previous
run surfaces as a toast on next launch. `bug_report.yml` then asks for the file instead of a
terminal.

**Out of scope, deliberately:** a "verbose logging" toggle, and unclean-shutdown detection for
*non-panic* crashes (SIGSEGV in Mesa/ALSA, OOM-kill). The latter needs a boot sentinel, and the
app installs no SIGTERM handler — so a normal logout or reboot with Melodia open would report
itself as a crash on the next launch. It belongs with signal handling and single-instance
enforcement, as its own issue.

---

## The decision that shapes everything: replace `env_logger` with `flexi_logger`

The instinct here is a hand-rolled fan-out over two `env_logger::Logger`s, to avoid taking a new
dependency. **The actual dependency graph reverses that**, and the numbers are the whole argument:

| | crates *added* to the build | crates *removed* |
|---|---|---|
| `flexi_logger` `default-features = false, features = ["colors"]` | **`nu-ansi-term`** (its only non-shared dep — `chrono`, `log`, `thiserror` are already direct deps of this crate, and flexi_logger asks for `chrono` with `default-features = false, features = ["clock"]`, a subset of the `clock` + `std` already declared) | `env_logger`, `env_filter`, `jiff`, `jiff-static`, `portable-atomic-util`, `anstream`, `anstyle-parse`, `anstyle-query`, `anstyle-wincon`, `colorchoice`, `is_terminal_polyfill`, `once_cell_polyfill`, `utf8parse` |

Measured by re-resolving `Cargo.lock` after the swap, not predicted: **13 crates out, 2 in**
(`flexi_logger` itself plus `nu-ansi-term`) — a net reduction of eleven, the opposite of what the
"minimal deps" instinct predicts. `jiff` and `anstream` were in the graph **solely** for
`env_logger`. `textfilter` (the `regex` feature) stays off, which is
exactly the trap the current `env_logger` comment at `Cargo.toml:246-250` was written to avoid; the
rationale transfers verbatim to the new dep line, and `cargo info flexi_logger` confirms
`default = [colors, textfilter]`, so `default-features = false` is what turns it off.

**Two crates that look like they leave and don't**, both worth stating so the tally isn't
"corrected" back later: `portable-atomic` stays (`i-slint-core`, `vtable` and `once_cell` all
depend on it) and `anstyle` stays (`annotate-snippets` ← `i-slint-compiler`). And `humantime` is
not in `Cargo.lock` at all — env_logger 0.11's `humantime` *feature* is implemented over `jiff`,
which is where that whole sub-tree comes from.

**`log` is not a third option to weigh against these two.** It is the *facade* — the `info!` /
`warn!` / `error!` macros and the `Log` trait every one of the 513 call sites uses — and
`flexi_logger` is a backend *for* it: `impl log::Log for FlexiLogger`, registered through
`log::set_boxed_logger`, exporting no logging macros of its own and depending on `log`
non-optionally. `env_logger` stood in exactly the same relation. Dropping the direct dep would
leave 513 call sites with nothing to call and wouldn't even remove the crate — 54 others in the
lock (sqlx, all eleven symphonia crates, reqwest, notify, lofty, rfd, femtovg, mio, tracing…)
need it regardless, and their records would keep arriving in our sink while ours could not.

Then it happens to provide, off the shelf, every piece this feature needs:

- `Criterion::AgeOrSize(Age::Day, N)` — **size** *and* age rotation. `tracing-appender` has
  time-based rotation only (`MINUTELY`/`HOURLY`/`DAILY`/`WEEKLY`/`NEVER`, confirmed on docs.rs), so
  one chatty session or a runaway `warn` loop produces an unbounded daily file. That alone rules it
  out for a desktop app, before the 513-call-site migration or `env-filter` dragging `regex` back in.
- `Cleanup::KeepLogFiles(n)` + `cleanup_in_background_thread(false)` — retention with **no extra
  thread**.
- `duplicate_to_stderr(Duplicate::All)` — one spec, two sinks, nothing to keep in sync.
- `WriteMode::Direct` — unbuffered. It is also flexi_logger's **default**, so this call documents
  an intent rather than changing a behaviour — but the intent is load-bearing and the call is what
  makes it reviewable: `main()` ends in `process::exit(0)`, and every buffered/async mode
  (`BufferAndFlush`, `tracing-appender`'s `non_blocking` + `WorkerGuard`) drops the tail — i.e.
  loses precisely the lines before a crash.
- `adaptive_format_for_stderr(AdaptiveFormat::Default)` — colour **only when stderr is a tty**.
  Not a nicety: env_logger is currently pulled with `auto-color`, which does exactly this, so a
  plain `format_for_stderr(colored_default_format)` would be a regression that fills
  `melodia 2>&1 | less` with escape sequences.
- `LoggerHandle::existing_log_files(&LogfileSelector)` — the diagnostics bundle gets its file list
  without a hand-rolled glob. Note the argument; the selector is what picks current-vs-rotated.
- `LoggerHandle::set_new_spec()` — a future verbose-logging toggle is already free.

`flexi_logger 0.31.9` (confirmed latest via `cargo search`), `rust-version = "1.87.0"` — under the
1.97 pin. `fern` and `log4rs` were considered and rejected: `fern`'s rotation is date-only (so size
is still hand-rolled — a dep *and* the code), `log4rs` is config-file-oriented with a much larger
tree for nothing gained here.

### The default spec is scoped, and that is not a detail

`env_logger::init()` with `RUST_LOG` unset floors at **`error`** — which is why, of the 513 `log::`
call sites in this tree, only the 7 `log::error!` ones can print today. Moving to a bare
`try_with_env_or_str("info")` would not merely turn our own logging on; it sets `info` as the floor
for **every crate in the graph** — slint, symphonia, zbus, ksni, notify, reqwest, souvlaki, rodio —
whose volume is decided by their choices, not ours, against a 10 MiB rotation budget.

So the default is **`"warn, melodia=info, Melodia=info"`**: the full app narrative, plus dependency
*warnings*, and no dependency chatter. Both tokens are needed and neither is a typo — `cargo
metadata` gives this package a lib target named `melodia` (all of `src/**`) and a bin target named
`Melodia` (`main.rs`'s own `log::info!` calls); `melodia-ui/src` contains no log calls at all.
`RUST_LOG` still overrides the whole string.

sqlx is the near-miss worth recording: its `LogSettings::default()` is `statements_level: Debug`
(`sqlx-core-0.9.0/src/connection.rs:201-208`) and nothing here calls `log_statements`, so queries
stay quiet — but `slow_statements_level: Warn` at a 1 s threshold still lands, which is a line
worth having.

---

## Status

Working doc — keep the markers current, delete the file when the feature ships.

Phases 1–3 landed together. Two things shifted from what is written below, both
deliberately: `redact_home` sits in `services/mod.rs` beside `write_text_atomic_sync`
rather than inside `diagnostics`, because the crash report needs it first; and
`crash_report::system_facts` is `pub(crate)` there rather than in a module of its own,
since `diagnostics` already depends on `crash_report` for the embedded reports.
`queries::track::count_tracks` is new — the library-shape line had no query to ask.

- [x] **Phase 1** — the sink (`flexi_logger`, `Paths::logs_dir`, `services::logging`)
- [x] **Phase 2** — the panic hook (`services::crash_report`)
- [x] **Phase 3** — the diagnostics report (`services::diagnostics`)
- [ ] **Phase 4** — the Settings card (`ui::diagnostics` + `diagnostics-section.slint`)
- [ ] **Phase 5** — the "crashed last run" toast
- [ ] **Phase 6** — i18n catalogues, `bug_report.yml`, `README.md`, `CLAUDE.md`

## Phase 1 — the sink

**`Cargo.toml`** — drop `env_logger`; add
`flexi_logger = { version = "0.31.9", default-features = false, features = ["colors"] }`.
Rewrite the comment block at `Cargo.toml:246-252` to carry the same warning forward: `textfilter`
is `flexi_logger`'s `regex` feature and stays off; note the `jiff`/`anstream` removal so nobody
"restores" `env_logger` later.

**`src/config.rs`** — one field, `pub logs_dir: PathBuf` (`data_dir.join("logs")`), joining the
subdirectory `create_dir_all` group at `config.rs:37-39` (there is a fourth at `:32` for
`data_dir` itself, which is not part of that group). Obliges an edit to
**`src/test_support.rs::paths_in`** (`test_support.rs:136`), which names every field *and*
pre-creates every directory — its own doc comment says that is why it is shared.

Log files and crash reports share **one** directory, so "Open log folder" is one button showing
everything a reporter needs. They can't collide: crash reports use a `crash-` prefix *and* a `.txt`
suffix, where `flexi_logger`'s cleanup matches on its `melodia` basename and `log` suffix — two
independent gates, and `.txt` opens in a text editor on double-click, which is what the hand-over
wants anyway.

**`src/services/logging.rs`** (new) — `pub fn install(paths: &Paths) -> AppResult<()>`:

```rust
Logger::try_with_env_or_str(DEFAULT_LOG_SPEC)?   // RUST_LOG still wins
    .log_to_file(FileSpec::default().directory(&paths.logs_dir)
                                    .basename("melodia").suppress_timestamp())
    .append()                                    // load-bearing — see below
    .rotate(Criterion::AgeOrSize(Age::Day, MAX_LOG_BYTES),
            Naming::Numbers, Cleanup::KeepLogFiles(KEEP_LOG_FILES))
    .cleanup_in_background_thread(false)
    .write_mode(WriteMode::Direct)
    .duplicate_to_stderr(Duplicate::All)
    .format_for_files(detailed_format)           // timestamp, level, module, file:line
    .adaptive_format_for_stderr(AdaptiveFormat::Default)
    .start()?
```

`.append()` is the one call whose absence silently defeats the feature: without it a restart
truncates, so the run that crashed is gone by the time the user opens the folder.

Store the returned `LoggerHandle` in a private `static OnceLock<LoggerHandle>` — `flexi_logger`
requires it stay alive, and `process::exit(0)` means a `main`-local binding would be fine but
unreachable from the two places that need it. Expose `pub fn flush()` and
`pub fn log_files() -> Vec<PathBuf>` over it, the latter swallowing
`existing_log_files`' error to an empty `Vec` — a diagnostics bundle is best-effort.

Sizing: `MAX_LOG_BYTES = 2 MiB`, `KEEP_LOG_FILES = 5` → ≤ 10 MiB ceiling in the data dir, named
constants beside a one-line justification.

**`src/main.rs`** — move `let paths = Paths::resolve()?;` (currently `main.rs:164`) **up** to just
before the current `env_logger::init()` at `main.rs:144`, and replace that line with
`services::logging::install(&paths)?`. The move is safe: `resolve` only touches `dirs::data_dir()`
and `create_dir_all`, spawns no thread, and it stays *after* the `mallopt` block, whose "literal
first statement" contract (`main.rs:92-93`, `CLAUDE.md:214`) is unaffected. The `--version` branch
already returns before any of this. Two consequences to handle rather than discover:

- **The `mallopt` comment names `env_logger::init()` by name** (`main.rs:92-93`) as an allocating
  call that justifies staying first. Swapping the logger makes that sentence false; the
  replacement allocates *more*, so the rationale strengthens and the comment just needs the new
  name.
- A `Paths::resolve()` failure now surfaces **before any logger exists**. It is a
  `dirs::data_dir()`-level catastrophe and there is nothing useful to log it to, so this stays
  as-is — noted only so it isn't mistaken for an oversight later.

**`src/shutdown.rs` / `src/main.rs` exit path** — call `services::logging::flush()` before
`shutdown::respawn_if_requested()` (which `exec()`s and never returns) and therefore before
`process::exit(0)`. Belt-and-braces under `WriteMode::Direct`, and mandatory the moment anyone
changes the write mode.

---

## Phase 2 — the panic hook

**`src/services/crash_report.rs`** (new). `pub fn install_hook(logs_dir: &Path)`, called from
`main.rs` immediately after `logging::install`, before the tokio runtime and before Slint, so boot
panics are covered.

Chain rather than replace — `panic::take_hook()` then `set_hook(Box::new(move |info| { … ; prev(info); }))`.
`panic::update_hook` is still `#[unstable(feature = "panic_update_hook")]` on the pinned 1.97
toolchain (verified in the toolchain's `std/src/panicking.rs`), so the take-then-set pair is the
only stable spelling. Chaining keeps the default stderr message intact for `cargo run`.

Confirmed against `library/std/src/panicking.rs` on the pinned toolchain: `rust_panic_with_hook`
invokes `Hook::Custom` *before* `__rust_start_panic`, so **the hook runs under `panic = "abort"`**.

Body, in this order, and the order matters:

1. Re-entrancy guard (`static AtomicBool`), so a panic inside the hook can't loop.
2. Write the report with plain `std::fs` — **no logger involvement**, so a panic that happened
   inside a logging call can't deadlock before the artifact exists.
3. `log::error!` a one-line summary, so the panic also lands at the end of the rolling log in
   sequence with whatever preceded it.
4. `prev(info)`.

The writer swallows every I/O error and cannot itself panic.

Report body — `crash-<YYYYMMDD-HHMMSS>.txt`, built by a **pure** `format_report(...) -> String`
that takes its inputs as arguments so it is testable without panicking:

```
Melodia crash report
version   : 0.10.0                       env!("CARGO_PKG_VERSION")
timestamp : 2026-08-06T14:30:00+02:00    chrono::Local::now() — already a direct dep
os / arch : linux / x86_64               std::env::consts
session   : wayland / KDE                XDG_SESSION_TYPE / XDG_CURRENT_DESKTOP (Linux)
install   : linux-x86_64-appimage        services::updater::target::current_target_key()
thread    : melodia-bg                   std::thread::current().name()
location  : src/ui/foo.rs:123            info.location()
panic     : …                            info.payload_as_str() (stable since 1.81)

backtrace
<std::backtrace::Backtrace::force_capture()>
```

`current_target_key()` returns **`Option<&'static str>`** — `None` on macOS and unsupported
arches — so it needs a fallback string, not an unwrap.

**Local time here is a deliberate first for this tree**, and the module doc should say so:
everything persisted today is UTC through `utils.rs:5` (`chrono::Utc::now().to_rfc3339()`), and
there is no `chrono::Local` anywhere. The reason to break the convention is that these two
artifacts sit in one folder and get read together — flexi_logger stamps its lines in local time by
default, so a UTC crash report would disagree with the log lines around it by the offset, and a
reporter's "it crashed around 2pm" would match neither the filename nor the header. The RFC-3339
offset keeps it unambiguous across reporters. The alternative — `.use_utc()` on the `Logger` plus
the existing helper — is equally consistent and was rejected only for the reporter's sake.

`force_capture` ignores `RUST_BACKTRACE`, which is the point. Set expectations honestly in the
module doc: `strip = "debuginfo"` keeps the symbol table, so frames carry **function names but no
file/line**, and `lto = "fat"` + `codegen-units = 1` inline aggressively so the trace is sparse.
v0 mangling (default since 1.97) at least makes the names unambiguous across monomorphizations.
The `log`-macro `file:line` in the rolling log is what fills that gap — which is why
`format_for_files(detailed_format)` above is not cosmetic.

Retention mirrors **`src/database/backup.rs`** exactly, which is this repo's settled discipline for
a delete sweep and should not be re-invented: one `fn file_name(ts) -> String` as the sole
definition of the scheme (`backup.rs:69-72`), its inverse `fn timestamp_of(name) -> Option<...>`
(the `version_of` role at `:83-85` — note the names there are `file_name(version: i64)` /
`version_of`, keyed on a *schema version* rather than a timestamp; the discipline transfers, the
identifiers don't), and a `prune` that collects **only** names that parse back (`:239`) and sorts
on the parsed key rather than trusting `read_dir` order or mtime. Anything else in the folder —
flexi_logger's own `melodia_r*.log` included, and a file the user put there most of all — is not
ours to retire. `MAX_CRASH_REPORTS` gets a justification comment in the shape of `MAX_BACKUPS`'
(`:48-53`).

---

## Phase 3 — the diagnostics report

**`src/services/diagnostics.rs`** (new). `pub async fn build_report(state: &AppState) -> AppResult<String>`,
composed of:

- the same header block as the crash report — **share one `system_facts()` builder**, don't write
  it twice;
- library shape: track count and watched-folder count (bug_report.yml already asks for library
  size);
- a fixed **allowlist** of settings that reproduce bugs — theme id/variant, locale, native
  titlebar, tray, crossfade/EQ/ReplayGain on-off, updater channel. Never a whole-struct dump, and
  **never `scrobble_credentials.json`**;
- the newest crash reports (up to N, truncated);
- the tail of `logging::log_files()`, newest first, capped at a total byte budget.

Everything goes through `fn redact_home(&str) -> Cow<str>` replacing `$HOME` with `~`. Audited:
no `log::` call site in the tree interpolates a token or session key, so the log tail is safe to
include — but that is a property to keep, and the module doc should say so.

Written with the existing `services::write_text_atomic_sync` (`services/mod.rs:114`). There is
**no clipboard support anywhere in the tree** (verified) and adding `arboard` would mean an X11
selection-owner thread for the life of the process — a file is both cheaper and what a GitHub
issue actually accepts.

---

## Phase 4 — the Settings card

**`melodia-ui/ui/views/settings/diagnostics-section.slint`** (new), copied structurally from
`about-section.slint` — the `row-visible(label, desc)` wrapper, one `show-*` per row ORed into
`out property has-matches`, the `VerticalLayout` wrapper so the root's preferred height tracks the
card, `SectionDivider` between rows. **Read that gating before copying it**: the first divider
(`about-section.slint:54`) is `show-app && show-author`, but the second (`:66`) is
`(show-app || show-author) && show-license` — a *cumulative* OR over everything preceding, not the
immediate neighbour. Copied as "both neighbours", a third row draws a leading divider whenever only
rows 1 and 3 match. Two rows, each `SettingRow` + child `SectionButton` (the control is a child,
not a property — there is no `ActionRow` in this codebase):

- **Logs** → `Open Folder`
- **Diagnostics report** → `Save…`

**`melodia-ui/ui/views/settings/pages/about-page.slint`** — mount it in column 1 under
`AboutSection` (`about-page.slint:34`), with `tab-name: @tr("About");` and add it to the page's
`has-matches` OR at `:13`. Column count stays 2 and the mount adds no `grid-row(` call, so
`ui::settings_page::tests::every_column_takes_its_own_cell` is unaffected, and
`every_mounted_section_carries_its_tab_name` covers the new mount for free — **provided the mount
is written on one line**, since that scan is line-based (`settings_page_tests.rs:198` filters
`lines()` for `"Section {"` and asks the same line for `tab-name:`).

**`melodia-ui/ui/settings.slint`** — two callbacks on the `Settings` global
(`open-log-folder()`, `save-diagnostics-report()`), beside `open-repository`.

**`src/ui/diagnostics.rs`** (new) — wires both, modelled line for line on **`src/ui/about.rs`**,
which is the exact precedent: `open::that` (already a dep, `open = "5.4.0"`) on the blocking pool
via `runtime.spawn(async { spawn_blocking(...).await })` (`about.rs:37`). The save action follows
`callbacks/playlists/files/export.rs:95-107` for its **dialog** half —
`slint::spawn_local(Compat::new(...))` with `rfd::AsyncFileDialog` and
`.set_parent(&ui.window().window_handle())` (mandatory per `ui-patterns.md`) — then a
`success`/`error` toast through the `NotificationsUi` the callback already holds (`:113-155`).
It is **not** the precedent for the write: `export.rs` contains no `spawn_blocking` at all, because
the fn it calls is already async. `write_text_atomic_sync` is not, so that hop comes from
`about.rs` instead.

Wired from `boot::ui_setup.rs` next to `ui::about::install(app, state)` (`ui_setup.rs:288`).
`notifications` already exists at `:285`, and `ui::file_watching::install(app, state, &notifications)`
at `:286` is the in-tree precedent for taking it as a third argument.

---

## Phase 5 — the "crashed last run" toast

Reuses the updater's pattern wholesale (`src/ui/updater_settings.rs:88-129`).

- Three `pure callback`s on the `Settings` global wrapping `@tr` literals
  (`crash-report-title` / `-message` / `-action-label`) — Slint 1.16 exposes no Rust-callable
  `tr()`, so this is the only way a Rust-pushed toast resolves in the running locale.
- One `else if (kind == "crash-report")` branch in the `Notifications.action` dispatcher at
  `melodia-ui/ui/globals/updater.slint:71-79`, calling `Settings.open-log-folder()` and then
  `Notifications.dismiss(id)` as both existing branches do. **The branch's string and the pushed
  `action_kind` are the same field and must match** — an earlier draft of this doc paired
  `action_kind: "crash-report"` with a `kind == "open-log-folder"` branch, which renders the button
  (`notification-stack.slint:102` gates only on a non-empty `action-label`) and does nothing when
  clicked, falling off the end of a dispatcher that has no `else`.
- On boot, after `notifications` exists in `main.rs`, `crash_report::take_unseen(logs_dir)` returns
  the newest report not yet acknowledged and pushes one `variant: "warning"` toast with
  `action_kind: "crash-report"`. Acknowledgement is a `last-seen` marker file the crash module
  owns inside `logs/`. It borrows the **atomic-write helpers** behind
  `scrobble_mbid_attempted.json` (`services::{load_json_or_default_sync, write_json_atomic_sync}`)
  and not its shape: that file is a set of ids under a declared `Paths` field, where this is a
  single high-water value the owning module resolves for itself. Either way no `settings.json` /
  `views.json` schema changes, since this is neither a user preference nor per-view UI state.

Zero false positives by construction: it fires only when a panic actually wrote a report.

---

## Phase 6 — i18n and docs

- **All six `.po` catalogues** (`melodia-ui/translations/{de,el,es,fr,it,tr}/LC_MESSAGES/melodia-ui.po`)
  get every new `@tr` msgid — roughly nine strings. This is not optional:
  `ui::locale::tests::every_translated_literal_has_a_msgid_in_every_catalogue` walks the whole
  `.slint` tree and fails the build on a miss.
- **`.github/ISSUE_TEMPLATE/bug_report.yml`** — the `logs` textarea is L95-103; replace the
  `RUST_LOG=info melodia` instruction at **L100** with "Settings → About → Diagnostics →
  **Save…**, then attach the file", and drop the "Windows release builds run without a console"
  concession at **L101**, which the feature retires. Keep `render: shell` (L103) as the paste
  fallback.
- **`README.md`** — a short Troubleshooting section: where the logs live per platform, what the
  two buttons do, and what the file contains (so nobody attaches one blind).
- **`CLAUDE.md`** — a `services/` module-map entry for the three new modules, and one Conventions
  bullet stating the two prohibitions that could be violated from outside those files: *never log a
  credential or token* (the diagnostics bundle ships the log tail), and *the crash-report prune
  deletes only names that parse back into its own scheme* (`backup.rs`'s rule, second instance).

---

## Tests

Per-module `#[cfg(test)] #[path = "tests/<name>_tests.rs"] mod tests;` — the convention
`backup.rs:257` uses, **not** the inline `mod tests { … }` that `tasks/updater_daily.rs` happens to
use. No `unwrap`/`expect` (the crate-wide ban applies to tests, and `-D warnings` promotes the
`expect_used` warning to an error), `type TestResult = Result<(), Box<dyn std::error::Error>>`
for the filesystem ones.

- `crash_report::format_report` — pure over its inputs; asserts every field is present and that
  `$HOME` is redacted.
- Crash-report retention — the `src/database/tests/backup_tests.rs:106,125` pair, copied:
  `prune_keeps_the_newest_and_deletes_the_rest`, writing the good names **out of order** so a
  prune trusting `read_dir` order fails it (that doc comment at `:103` is the reason it exists);
  and `prune_never_touches_a_name_it_did_not_write`, whose decoy list follows the shape at
  `:127-135` — near-misses, not just unrelated files: `melodia_rCURRENT.log`, `melodia_r00000.log`,
  `crash-NOPE.txt` and a hand-made `notes.txt`.
- `take_unseen` — returns a report once, then not again; returns nothing on an empty folder.
- `redact_home` — replaces, and is a no-op with `HOME` unset, via
  `test_support::with_env_var` (`test_support.rs:300`). That is a one-line wrapper over
  `with_env_set` (`:252`), which holds the sole `#[allow(unsafe_code)]` and the binary's only
  `ENV_LOCK` — so a new wrapper **delegates**; locking and then calling one trips the reentrancy
  assert at `:210`, by design, into a named panic rather than a hang.
- `diagnostics::build_report` over a `tempfile::tempdir()` of fake logs — tail ordering, the byte
  budget, and that no non-allowlisted setting key appears in the output.

`logging::install` itself is not unit-tested — `log::set_boxed_logger` is once-per-process, and
`updater_daily`'s test module sets the precedent of testing only the pure helpers.

## Verification

1. `cargo clippy --all-targets --locked -- -D warnings` — the whole gate, both packages.
2. `cargo test --locked` — includes the i18n catalogue pin and the Settings-page pins.
3. `cargo build && target/debug/Melodia`, then check `~/.local/share/Melodia/logs/melodia_rCURRENT.log`
   exists and has boot lines **with no `RUST_LOG` set** — that is the whole point of the default
   spec. Confirm the scoping too: dependency *warnings* present, dependency chatter absent.
4. Restart and confirm the file **grew** rather than restarting from zero (proves `.append()`).
5. Force a panic behind a temporary debug-only trigger: confirm `crash-*.txt` appears with a
   backtrace, the default panic message still prints to stderr, and the next launch shows the toast
   exactly once. Verify again against a **release** build, which is where `panic = "abort"` and
   `strip = "debuginfo"` actually apply — this is the one step a debug build cannot answer.
6. Settings → About → Diagnostics: **Open Folder** opens the right directory; **Save…** writes a
   file whose paths read `~/…` and which contains no credential.
7. `RUST_LOG=debug` still overrides both sinks, and `target/debug/Melodia 2>&1 | cat` shows no ANSI
   escape sequences — that is what `adaptive_format_for_stderr` buys over a plain coloured format,
   and it is the behaviour env_logger's `auto-color` had.
8. `/usr/bin/time -v target/release/Melodia` once at the end — expected flat (one file handle, no
   new threads: `WriteMode::Direct` and `cleanup_in_background_thread(false)` are chosen for that).

## Known caveat to record, not solve

Nothing enforces a single instance, so two concurrent Melodias append to one log file. Records stay
intact (one `write_all` per line) but interleave, and a rotation could race. Not worth
per-process log files — that trades one readable artifact for a folder nobody can hand over. Note
it in the module doc; it closes for free if single-instance enforcement ever lands.

## Sources

- [flexi_logger on crates.io](https://crates.io/crates/flexi_logger) · [repository](https://github.com/emabee/flexi_logger) · [code examples](https://docs.rs/flexi_logger/latest/flexi_logger/code_examples/index.html)
- [tracing_appender::rolling::Rotation](https://docs.rs/tracing-appender/latest/tracing_appender/rolling/struct.Rotation.html) — time-based only
- [Logging in Rust — Shuttle](https://www.shuttle.dev/blog/2023/09/20/logging-in-rust)
