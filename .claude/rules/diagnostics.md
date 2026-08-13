---
paths:
  - src/services/logging.rs
  - src/services/crash_report.rs
  - src/services/diagnostics.rs
  - src/services/mod.rs
  - src/ui/settings/diagnostics.rs
  - src/main.rs
---

# The diagnostics trio — logging, crash reports, the bug-report bundle

Three files answer to a bug report rather than to a subsystem, and they are read together.

- **`logging.rs`** — the `flexi_logger` sink: `logs/melodia_rCURRENT.log` on every run with no env var set, 2 MiB × 7 rotated files, counted rather than dated so an occasional user's history isn't swept at startup, and `.append()`, the one call whose absence silently erases the run that crashed. `RUST_LOG` overrides the scoped default (a `warn` floor, our two targets `melodia`/`Melodia` at `info`, plus a per-module `error` mute for `symphonia_bundle_mp3::layer3`'s per-frame bit-reservoir underflow and `sctk_adwaita::buttons` misparsing a GNOME portal key). **Mutes are scoped by module, never by crate** — a crate-wide directive takes unrelated warnings with it; `SPEC_TAIL` argues each. **The spec is *built*, not written down**: `spec_for(level)` folds floor + targets + tail around one level, so the Settings → About → Diagnostics **Verbose logging** switch (`logging::set_verbose`, live via `LoggerHandle::set_new_spec`, no relaunch) can raise to `debug` without a second spelling drifting — and the mutes ride along, `layer3` otherwise burying the detail the switch was flipped for. The flag persists (`DiagnosticsFlags::verbose_logging`) and `install` reads it **before anything else runs** so a bad boot is captured; a set `RUST_LOG` wins outright and `set_verbose` declines and says so. **What belongs at which level is argued in the module's own doc** — read it before adding a `log::` call.
- **`crash_report.rs`** — the *chained* panic hook (it does fire under `panic = "abort"`), writing `crash-<stamp>.txt` with plain `fs` and never through the logger, so a panic raised inside a logging call still leaves an artifact; plus `take_unseen`, the once-per-crash gate behind the boot toast.
- **`diagnostics.rs`** — `build_report`, the one text file a reporter attaches. Reached from the UI only by `ui/settings/diagnostics.rs`.

All three stamp **local** time where everything else persisted is UTC (`utils::now_rfc3339`) — they share a folder with `flexi_logger`'s local-stamped lines and get read together.

## Four rules keep the trio a diagnostic rather than a liability, each one edit from inverting

1. **`logging::install` returns nothing.** Opening the file can fail for reasons that aren't the app's (a root-owned `melodia_rCURRENT.log`, a full disk), and a `?` there refused to *start* Melodia, explaining why to the stderr a `.desktop` launch discards. It degrades to stderr and records the reason for `unavailable_reason()`, which `diagnostics::log_section` prints so an empty logs section says which kind of empty it is. **The reason goes through `services::describe`, not `to_string()`** — every `FlexiLoggerError` `Display` arm is a static sentence and `OutputIo(#[from] io::Error)` never interpolates its source, so a bare `to_string()` reports a permissions failure and a full disk identically. `describe` walks `.source()` and lives on `services/mod.rs` beside `redact_home`, because `AppError`'s four I/O-boundary variants share that shape by construction — **any** `log::` call handed one with a bare `{e}` names the operation and drops the reason (`artist_images` is the second caller; a third belongs there, not on a local copy). **It skips a source the `Display` already printed**, since `AppError`'s three `#[from]` variants spell `#[error("… {0}")]` over the field `#[from]` also makes the source and sqlx nests the same shape — an unconditional walk reports one constraint failure three times. Pinned both directions by `services::tests::a_cause_is_appended_once_and_never_repeated` and `logging::tests`.
2. **`log_files()` is newest-first only because `newest_first` reverses the rotated half.** `LoggerHandle::existing_log_files` ends in a plain `sort()` that undoes `FileSpec::read_dir_related_files`' reverse, so the public API hands back *ascending* names and a higher `Naming::Numbers` index is the newer file. The bundle spends its byte budget down that list, so the wrong order costs the oldest log. Pinned by `logging::tests` over synthetic names; it has been wrong twice.
3. **A crash report is `head_of`, a log is `tail_of`** — a report opens with version/timestamp/thread/location/message and it's the backtrace that runs long.
4. **`services::redact_home` resolves through `dirs::home_dir()`, never `$HOME`** — that variable is unset on Windows, the one platform whose users have no console to have read the paths from, while `README.md` and `bug_report.yml` both promise `~`. Its pure half `redact_prefix` pins the Windows shape, no Linux runner reaching the other arm.

**Never log a credential, token or session key.** `build_report` ships the tail of the rolling log inside a file users attach to public GitHub issues; the settings block is a hand-written **allowlist**, so a new `SettingsData` field can't start shipping by accident, and `scrobble_credentials.json` is never read by that module.

**The sweep deletes only names that parse back into its own scheme.** `logs/` is shared, so `crash_report::prune` collects through `timestamp_of` (`crash-<stamp>.txt` and nothing else) and `flexi_logger`'s own cleanup is gated the other way, on its `melodia` basename and `log` suffix. Two independent gates — don't add a third file to that folder without checking it falls outside both.

**`--logs` is the branch that answers what this feature can't.** Everything above sits behind Settings → About → Diagnostics, unreachable when the thing being reported is that Melodia won't open, so `main()` prints `paths.logs_dir` and returns ahead of the database and Slint. It is a Linux/macOS route: `windows_subsystem = "windows"` leaves a release build with no console, so `GetStdHandle` hands back nothing and the `writeln!` is swallowed (`--version` escapes only because the updater spawns it with `Stdio::piped()`). Debug builds are console-subsystem, so the gap is invisible locally — hence `README.md` and `bug_report.yml` pointing Windows at `%APPDATA%\Melodia\logs\`. Attaching the parent console was declined: one diagnostic for a fourth Win32 FFI site `cfg(windows)` keeps out of CI forever.
