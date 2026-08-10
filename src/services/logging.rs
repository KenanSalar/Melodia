//! The file log sink.
//!
//! Every run writes to `logs/melodia_rCURRENT.log` with no env var set, because
//! the artifact only exists if it exists by default: a user launching from a
//! `.desktop` entry, the tray, or a Windows GUI-subsystem build has no console
//! to have captured stderr from, and "reproduce it with `RUST_LOG=info`" is
//! advice nobody follows after the crash they already had.
//!
//! Crash reports land in the same directory (`services::crash_report`) so
//! "Open log folder" hands a reporter everything at once. The two naming
//! schemes can't collide — this one owns the `melodia` basename and `log`
//! suffix, that one a `crash-` prefix and `.txt` — and each sweep is gated on
//! recognising its own names, so neither retires the other's files or a user's.
//!
//! **Two concurrent Melodias append to one file.** Nothing enforces a single
//! instance yet. Records survive (one `write_all` per line) but interleave, and
//! a rotation could race. Per-process files would trade the one artifact a
//! reporter can hand over for a folder they can't; this closes for free if
//! single-instance enforcement ever lands.

use std::path::PathBuf;
use std::sync::OnceLock;

use flexi_logger::{
    AdaptiveFormat, Age, Cleanup, Criterion, Duplicate, FileSpec, FlexiLoggerError, LogSpecification,
    LogfileSelector, Logger, LoggerHandle, Naming, WriteMode, detailed_format,
};

use crate::config::Paths;

/// Dependency warnings, our own narrative, and nothing else.
///
/// A bare `"info"` would set that floor for every crate in the graph — slint,
/// symphonia, zbus, notify, reqwest — whose volume is decided by their choices
/// against our rotation budget. Both tokens are real targets: `melodia` is the
/// lib (all of `src/`), `Melodia` the bin (`main.rs`). `RUST_LOG` overrides it.
///
/// `layer3` is muted because its bit-reservoir underflow warns once per *frame*
/// on any stream not opened at byte zero — every seek and every gapless preload
/// — while the decoder compensates and plays it fine. Safe to scope by module:
/// that is the only `warn!` in it, and the demuxer's all still land. Directives
/// match longest-name-first, so this outranks the floor.
///
/// `sctk_adwaita::buttons` is the same trade one dependency over. It reads
/// `org.gnome.desktop.wm.preferences button-layout` out of the XDG portal, KDE
/// answers with an empty left side, and both `warn!`s in the module are about
/// that parse — while the frame those buttons belong to is the client-side
/// titlebar winit builds and immediately hides whenever we ask for no
/// decorations. So it fires once per launch in custom-titlebar mode, naming
/// controls that are never drawn. By module rather than by crate: the crate's
/// other `warn!` is a glyph-rasterization failure and has nothing to do with
/// the portal.
const DEFAULT_LOG_SPEC: &str =
    "warn, melodia=info, Melodia=info, symphonia_bundle_mp3::layer3=error, sctk_adwaita::buttons=error";

/// Rotate at 2 MiB or at the turn of the day; `Cleanup` never counts the live
/// file, so the ceiling is 8 files, 16 MiB.
///
/// Counted rather than dated (`Cleanup::KeepForDays`): that variant drops the
/// byte bound, and its startup sweep wipes an occasional user's whole history
/// before they can report anything.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LOG_FILES: usize = 7;

/// Keeps the logger alive for the process. `flexi_logger` shuts its writers down
/// when the handle drops, and `main` ends in `process::exit(0)` — so a local
/// binding would work but be unreachable from [`flush`] and [`log_files`].
static HANDLE: OnceLock<LoggerHandle> = OnceLock::new();

/// Why the file sink isn't running, on the runs where it isn't. Read back by
/// [`unavailable_reason`] so a diagnostics bundle says *why* it carries no logs
/// rather than looking like an install that never wrote any.
static FILE_SINK_ERROR: OnceLock<String> = OnceLock::new();

/// Start logging. Call once, as early in `main` as a [`Paths`] exists.
///
/// **Infallible on purpose.** Opening the file can fail for reasons that have
/// nothing to do with the app — a `melodia_rCURRENT.log` left root-owned by one
/// `sudo melodia`, a full disk, exhausted descriptors — and a `?` here refused
/// to start Melodia over it, printing why to the stderr that a `.desktop` launch
/// discards. That is the failure this whole module exists to stop happening, so
/// it degrades to stderr-only instead. The panic hook writes through plain `fs`
/// and doesn't touch the logger, so crash reports survive either way.
pub fn install(paths: &Paths) {
    let error = match start_to_file(paths) {
        Ok(handle) => {
            let _ = HANDLE.set(handle);
            return;
        }
        // Flattened rather than `to_string()`d: every `FlexiLoggerError` `Display`
        // arm is a static sentence and `OutputIo` never interpolates the
        // `io::Error` it holds, so a root-owned log file and a full disk read
        // identically without the chain — and the chain is the whole reason this
        // reason is recorded at all.
        Err(e) => super::describe(&e),
    };

    if let Ok(handle) = base_logger().start() {
        let _ = HANDLE.set(handle);
    }
    let _ = FILE_SINK_ERROR.set(error);
    // After the fallback logger is up, so it lands somewhere rather than
    // vanishing into an uninitialized `log` facade.
    log::error!(
        "file logging unavailable, continuing on stderr only: {}",
        unavailable_reason().unwrap_or("unknown")
    );
}

/// The spec, the levels and the stderr half — everything the fallback keeps.
fn base_logger() -> Logger {
    // `try_with_env_or_str` falls back to the given spec when `RUST_LOG` is
    // malformed, so the only way this errors is a broken `DEFAULT_LOG_SPEC` —
    // which `tests::the_default_spec_parses_into_the_directives_it_spells`
    // pins. An unparseable literal then degrades to the `warn` floor that
    // spec's first directive sets, minus the app's own narrative, rather than
    // costing the process its logger. Not `LogSpecification::default()`, which
    // is `off()`.
    Logger::try_with_env_or_str(DEFAULT_LOG_SPEC)
        .unwrap_or_else(|_| Logger::with(LogSpecification::warn()))
        // Adaptive, not plain coloured: env_logger's `auto-color` suppressed
        // ANSI off a tty and piping the app somewhere shouldn't regress.
        .adaptive_format_for_stderr(AdaptiveFormat::Default)
}

fn start_to_file(paths: &Paths) -> Result<LoggerHandle, FlexiLoggerError> {
    base_logger()
        .log_to_file(
            FileSpec::default()
                .directory(&paths.logs_dir)
                .basename("melodia")
                .suppress_timestamp(),
        )
        // Without this a restart truncates, so the run that crashed is gone by
        // the time the user opens the folder — the one call whose absence
        // defeats the feature silently.
        .append()
        .rotate(
            Criterion::AgeOrSize(Age::Day, MAX_LOG_BYTES),
            Naming::Numbers,
            Cleanup::KeepLogFiles(KEEP_LOG_FILES),
        )
        .cleanup_in_background_thread(false)
        // The default, stated because it is load-bearing: `process::exit(0)`
        // runs no destructors, and every buffered mode drops the tail — which
        // is exactly the lines before a crash.
        .write_mode(WriteMode::Direct)
        .duplicate_to_stderr(Duplicate::All)
        .format_for_files(detailed_format)
        .start()
}

/// Why there are no log files, or `None` when the file sink is running.
pub fn unavailable_reason() -> Option<&'static str> {
    FILE_SINK_ERROR.get().map(String::as_str)
}

/// Flush before an exit path that runs no destructors. A no-op under
/// [`WriteMode::Direct`], and the thing that saves the tail if that ever changes.
pub fn flush() {
    if let Some(handle) = HANDLE.get() {
        handle.flush();
    }
}

/// The current log file and its rotated siblings, newest first.
///
/// Best-effort: a diagnostics bundle missing a file is worth more than one that
/// failed to build, so an unreadable directory reads as no logs.
pub fn log_files() -> Vec<PathBuf> {
    let Some(handle) = HANDLE.get() else {
        return Vec::new();
    };
    // Two calls, because asked for together the live file is *appended* after
    // the rotated ones and no single ordering of that list is meaningful.
    let current = handle
        .existing_log_files(&LogfileSelector::none().with_r_current())
        .unwrap_or_default();
    let rotated = handle
        .existing_log_files(&LogfileSelector::default())
        .unwrap_or_default();
    newest_first(current, rotated)
}

/// Join the live file and the rotated ones into one newest-first list.
///
/// The reversal is the whole of it, and it is not what reading `FileSpec`
/// suggests: that sorts ascending and then *reverses*, but `LoggerHandle`'s own
/// `existing_log_files` ends with a plain `sort()` which undoes it — so the
/// public API hands back ascending names, and under `Naming::Numbers` a higher
/// index is the newer file. Left alone, the byte budget in
/// `services::diagnostics` is spent on the *oldest* rotated log the moment the
/// live one is short, which is exactly the launch after a rotation.
///
/// Split out so the ordering is testable without a live logger; it has been
/// wrong twice with nothing to catch it.
fn newest_first(current: Vec<PathBuf>, mut rotated: Vec<PathBuf>) -> Vec<PathBuf> {
    rotated.reverse();
    let mut files = current;
    files.extend(rotated);
    files
}

#[cfg(test)]
#[path = "tests/logging_tests.rs"]
mod tests;
