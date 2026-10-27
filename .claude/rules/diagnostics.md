---
paths:
  - crates/melodia-platform/src/services/platform/logging.rs
  - crates/melodia-platform/src/services/platform/crash_report.rs
  - crates/melodia-app/src/services/diagnostics.rs
  - crates/melodia-core/src/utils/redact.rs
  - crates/melodia-views/src/ui/settings/diagnostics.rs
  - crates/melodia/src/main.rs
---

# The diagnostics trio — logging, crash reports, the bug-report bundle

Three files answer to a bug report rather than to a subsystem, and they are read together.

- **`logging.rs`** — the `flexi_logger` sink, written every run with no env var set. The rotation,
  `.append()`, the built spec and its per-module mutes are each argued at their definitions there,
  and **what belongs at which level is argued in the module's own doc**: read it before adding a
  `log::` call. The Settings → About → Diagnostics **Verbose logging** switch reaches it through
  `logging::set_verbose`, live, the flag persisting as `DiagnosticsFlags::verbose_logging`; a set
  `RUST_LOG` wins outright.

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
inside a file users attach to public GitHub issues; what else it may carry is argued in its module
doc.

**The sweep deletes only names that parse back into its own scheme** (root `CLAUDE.md`):
`crash_report::prune` gates on `timestamp_of`, `flexi_logger`'s own cleanup on its `melodia`
basename and `log` suffix — two independent gates, so a third file in `logs/` has to fall outside
both.

**`--logs` is the branch that answers what this feature can't** — everything above sits behind
Settings → About → Diagnostics, unreachable when the thing being reported is that Melodia won't
open. It is a Linux/macOS route: under `windows_subsystem = "windows"` a release build has no
console, so `GetStdHandle` hands back nothing and the `writeln!` is swallowed (`--version` escapes
only because the updater spawns it with `Stdio::piped()`). Attaching the parent console was
declined: one diagnostic is not worth another Win32 FFI site.
