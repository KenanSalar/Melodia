---
paths:
  - src/services/logging.rs
  - src/services/crash_report.rs
  - src/services/diagnostics.rs
  - src/utils/redact.rs
  - src/ui/settings/diagnostics.rs
  - src/main.rs
---

# The diagnostics trio — logging, crash reports, the bug-report bundle

Three files answer to a bug report rather than to a subsystem, and they are read together.

- **`logging.rs`** — the `flexi_logger` sink: `logs/melodia_rCURRENT.log` every run with no env var
  set, 2 MiB × 7 rotated files, counted rather than dated so an occasional user's history isn't
  swept at startup, and `.append()`, the one call whose absence silently erases the run that
  crashed. `RUST_LOG` overrides the scoped default — a `warn` floor, our two targets
  `melodia`/`Melodia` at `info`, plus a per-module `error` mute for
  `symphonia_bundle_mp3::layer3`'s per-frame bit-reservoir underflow and `sctk_adwaita::buttons`
  misparsing a GNOME portal key. **Mutes are scoped by module, never by crate** — a crate-wide
  directive takes unrelated warnings with it; `SPEC_TAIL` argues each. **The spec is *built*, not
  written down**: `spec_for(level)` folds floor + targets + tail around one level, so the
  Settings → About → Diagnostics **Verbose logging** switch (`logging::set_verbose`, live via
  `LoggerHandle::set_new_spec`, no relaunch) raises to `debug` without a second spelling drifting,
  and the mutes ride along — `layer3` otherwise buries the detail the switch was flipped for. The
  flag persists (`DiagnosticsFlags::verbose_logging`) and `install` reads it **before anything else
  runs** so a bad boot is captured; a set `RUST_LOG` wins outright and `set_verbose` declines and
  says so. **What belongs at which level is argued in the module's own doc** — read it before
  adding a `log::` call.

- **`crash_report.rs`** — the *chained* panic hook (it does fire under `panic = "abort"`), writing
  `crash-<stamp>.txt` with plain `fs` and never through the logger, so a panic raised inside a
  logging call still leaves an artifact; plus `take_unseen`, the once-per-crash gate behind the
  boot toast.

- **`diagnostics.rs`** — `build_report`, the one text file a reporter attaches. Reached from the UI
  only by `ui/settings/diagnostics.rs`.

All three stamp **local** time where everything else persisted is UTC (`utils::now_rfc3339`) — they
share a folder with `flexi_logger`'s local-stamped lines and get read together.

Four decisions keep the trio a diagnostic rather than a liability, and each is argued at its own
definition rather than here: `logging::install` returns nothing (infallible on purpose, degrading
to stderr rather than refusing to boot); `log_files()` is newest-first only because `newest_first`
reverses the rotated half; a crash report is `head_of` where a log is `tail_of`; and
`error::describe` is what a `log::` call hands an error to, never `to_string()`. Read those four
doc comments before changing any of them.

**Never log a credential, token or session key.** `build_report` ships the tail of the rolling log
inside a file users attach to public GitHub issues; its settings block is a hand-written
**allowlist**, so a new `SettingsData` field can't start shipping by accident, and
`scrobble_credentials.json` is never read by that module.

**The sweep deletes only names that parse back into its own scheme** (root `CLAUDE.md`):
`crash_report::prune` gates on `timestamp_of`, `flexi_logger`'s own cleanup on its `melodia`
basename and `log` suffix — two independent gates, so a third file in `logs/` has to fall outside
both.

**`--logs` is the branch that answers what this feature can't** — everything above sits behind
Settings → About → Diagnostics, unreachable when the thing being reported is that Melodia won't
open. It is a Linux/macOS route: under `windows_subsystem = "windows"` a release build has no
console, so `GetStdHandle` hands back nothing and the `writeln!` is swallowed (`--version` escapes
only because the updater spawns it with `Stdio::piped()`). Attaching the parent console was
declined — one diagnostic for a fourth Win32 FFI site `cfg(windows)` keeps out of CI forever.
