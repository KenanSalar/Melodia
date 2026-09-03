//! The file log sink.
//!
//! Every run writes `logs/melodia_rCURRENT.log` with no env var set: a `.desktop`, tray or Windows
//! GUI launch leaves no console stderr could have been captured from, and nobody re-runs under
//! `RUST_LOG` after the crash they already had. The sink's shape — rotation, `RUST_LOG`
//! precedence, the two mutes, the newest-first ordering — is `.claude/rules/diagnostics.md`.
//!
//! # Which level
//!
//! - **`info`** — happened once, matters at a glance. On for every user, so the rotation budget is
//!   sized against its volume.
//! - **`warn`** — a user could notice. Expected, self-recovering and unbounded goes to `debug`
//!   with a count instead; per occurrence it only teaches the reader to skim warnings
//!   (`player::stream_health`).
//! - **`debug`** — what the user did, as a narrative: one line per class of action at the seam
//!   every path funnels through, and **nothing on a timer, frame or keystroke**. The *action*,
//!   never the widget — a play/pause line at the button misses the shortcut, the tray, the media
//!   keys and Now Playing. `trace` is unused, hence [`VERBOSE_LEVEL`] stopping here.
//!
//! **One writer, by `services::single_instance`'s doing** — a second launch forwards and returns
//! before `install` runs. Nothing here defends the file though: an unmakeable claim boots anyway,
//! and two Melodias would interleave with a rotation able to race.

use std::path::PathBuf;
use std::sync::OnceLock;

use flexi_logger::{
    AdaptiveFormat, Age, Cleanup, Criterion, Duplicate, FileSpec, FlexiLoggerError,
    LogSpecification, LogfileSelector, Logger, LoggerHandle, Naming, WriteMode, detailed_format,
};

use crate::config::Paths;

/// What our own two targets log at with Verbose Logging off.
const NORMAL_LEVEL: &str = "info";

/// …and with it on. Not `trace`: there are no `log::trace!` sites in the tree, so it would differ
/// only in what the muted dependencies say.
const VERBOSE_LEVEL: &str = "debug";

/// The two dependency modules that warn about something we already know: `layer3`'s bit-reservoir
/// underflow fires once per *frame* on any stream not opened at byte zero while the decoder
/// compensates and plays it fine, and `sctk_adwaita::buttons` names controls on the client-side
/// titlebar winit builds and immediately hides in custom-titlebar mode.
///
/// Scoped by module, never by crate: each is the only `warn!` in its own, so the siblings still
/// land. Directives match longest-name-first, so these outrank the floor. Shared by both specs —
/// a verbose spec dropping `layer3` buries the detail the switch was flipped for.
const SPEC_TAIL: &str = "symphonia_bundle_mp3::layer3=error, sctk_adwaita::buttons=error";

/// Dependency warnings, our own narrative at `level`, and nothing else. A bare `"info"` would
/// floor every crate in the graph, spending our rotation budget on their choices. Both tokens are
/// real targets: `melodia` the lib, `Melodia` the bin. `RUST_LOG` overrides it.
fn spec_for(level: &str) -> String {
    format!("warn, melodia={level}, Melodia={level}, {SPEC_TAIL}")
}

/// Rotate at 2 MiB or the turn of the day; `Cleanup` never counts the live file, so the ceiling is
/// 8 files. Counted rather than dated — `KeepForDays` drops the byte bound, and its startup sweep
/// wipes an occasional user's whole history before they can report anything.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LOG_FILES: usize = 7;

/// Keeps the logger alive — `flexi_logger` shuts its writers down when the handle drops. A local
/// binding would be unreachable from [`flush`] and [`log_files`].
static HANDLE: OnceLock<LoggerHandle> = OnceLock::new();

/// Why the file sink isn't running, so [`unavailable_reason`] can tell a diagnostics bundle that
/// apart from an install that never wrote any logs.
static FILE_SINK_ERROR: OnceLock<String> = OnceLock::new();

/// Whether `RUST_LOG` was **set** when [`install`] ran; if so it owns the spec and [`set_verbose`]
/// declines. Presence, not a successful parse — a malformed value is still a developer saying
/// which levels they want, and a GUI switch quietly taking the spec back from a typo is the
/// surprise.
static ENV_SPEC_WINS: OnceLock<bool> = OnceLock::new();

/// Start logging. Call once, as early in `main` as a [`Paths`] exists.
///
/// **Infallible on purpose** — `.claude/rules/diagnostics.md` argues it; it degrades to
/// stderr-only rather than refusing to start Melodia over a log file.
///
/// Reads the Verbose Logging flag itself rather than taking it as an argument: applied later from
/// the UI, the whole boot would stay at [`NORMAL_LEVEL`], and a boot going wrong is the window the
/// switch is worth having.
pub fn install(paths: &Paths) {
    let _ = ENV_SPEC_WINS.set(std::env::var_os("RUST_LOG").is_some());

    // An unparseable settings file is no reason to start louder; it surfaces later through
    // `AppState::init`'s own read.
    let verbose = super::settings::read_settings(paths)
        .is_ok_and(|settings| settings.diagnostics.verbose_logging);
    let spec = spec_for(if verbose { VERBOSE_LEVEL } else { NORMAL_LEVEL });

    let error = match start_to_file(paths, &spec) {
        Ok(handle) => {
            let _ = HANDLE.set(handle);
            return;
        }
        // Not `to_string()`: `OutputIo` never interpolates its `io::Error`, so without the chain a
        // root-owned file and a full disk read identically.
        Err(e) => crate::error::describe(&e),
    };

    if let Ok(handle) = base_logger(&spec).start() {
        let _ = HANDLE.set(handle);
    }
    let _ = FILE_SINK_ERROR.set(error);
    // After the fallback logger, or it vanishes into an uninitialized facade.
    log::error!(
        "file logging unavailable, continuing on stderr only: {}",
        unavailable_reason().unwrap_or("unknown")
    );
}

/// The spec, the levels and the stderr half — everything the fallback keeps.
fn base_logger(spec: &str) -> Logger {
    // `try_with_env_or_str` falls back to `spec` on a malformed `RUST_LOG`, so only a broken
    // `spec_for` errors here. The `warn` floor costs the app's narrative, not the process's
    // logger; `default()` is `off()`, which would cost both.
    Logger::try_with_env_or_str(spec)
        .unwrap_or_else(|_| Logger::with(LogSpecification::warn()))
        // Adaptive rather than plain coloured: piping the app somewhere shouldn't regress.
        .adaptive_format_for_stderr(AdaptiveFormat::Default)
}

fn start_to_file(paths: &Paths, spec: &str) -> Result<LoggerHandle, FlexiLoggerError> {
    base_logger(spec)
        .log_to_file(
            FileSpec::default().directory(&paths.logs_dir).basename("melodia").suppress_timestamp(),
        )
        // Without this a restart truncates, so the run that crashed is gone by the time the folder
        // is opened — the one call whose absence defeats the feature silently.
        .append()
        .rotate(
            Criterion::AgeOrSize(Age::Day, MAX_LOG_BYTES),
            Naming::Numbers,
            Cleanup::KeepLogFiles(KEEP_LOG_FILES),
        )
        .cleanup_in_background_thread(false)
        // The default, stated because it is load-bearing: `process::exit(0)` runs no destructors,
        // and a buffered mode drops exactly the lines before a crash.
        .write_mode(WriteMode::Direct)
        .duplicate_to_stderr(Duplicate::All)
        .format_for_files(detailed_format)
        .start()
}

/// Why there are no log files, or `None` when the file sink is running.
pub fn unavailable_reason() -> Option<&'static str> {
    FILE_SINK_ERROR.get().map(String::as_str)
}

/// Swap the running level, applied to the live sinks so no relaunch is needed.
///
/// **`RUST_LOG` wins and this declines**, out loud rather than silently: it is the developer
/// escape hatch, and a GUI switch fighting it would make the variable mean nothing.
pub fn set_verbose(on: bool) {
    if ENV_SPEC_WINS.get().copied().unwrap_or(false) {
        log::info!("verbose logging: RUST_LOG is set and takes precedence; leaving the spec alone");
        return;
    }
    let Some(handle) = HANDLE.get() else { return };
    let spec = spec_for(if on { VERBOSE_LEVEL } else { NORMAL_LEVEL });
    match LogSpecification::parse(&spec) {
        Ok(parsed) => {
            handle.set_new_spec(parsed);
            log::info!("verbose logging {}", if on { "enabled" } else { "disabled" });
        }
        // Unreachable while the spec test passes; keeping the level beats dropping to the floor.
        Err(e) => log::warn!("verbose logging: could not parse the spec '{spec}': {e}"),
    }
}

/// Flush before an exit path that runs no destructors. A no-op under [`WriteMode::Direct`], and
/// what saves the tail if that ever changes.
pub fn flush() {
    if let Some(handle) = HANDLE.get() {
        handle.flush();
    }
}

/// The current log file and its rotated siblings, newest first. Best-effort: a bundle missing a
/// file is worth more than one that failed to build, so an unreadable directory reads as no logs.
pub fn log_files() -> Vec<PathBuf> {
    let Some(handle) = HANDLE.get() else {
        return Vec::new();
    };
    // Two calls: asked together, the live file lands *after* the rotated ones and no single
    // ordering of that list is meaningful.
    let current =
        handle.existing_log_files(&LogfileSelector::none().with_r_current()).unwrap_or_default();
    let rotated = handle.existing_log_files(&LogfileSelector::default()).unwrap_or_default();
    newest_first(current, rotated)
}

/// Join the live file and the rotated ones into one newest-first list.
///
/// The reversal is the whole of it, and reading `FileSpec` argues against it:
/// `existing_log_files` ends in a plain `sort()` that undoes
/// `FileSpec::read_dir_related_files`' own reverse, so the public API hands back ascending names
/// and a higher `Naming::Numbers` index is the *newer* file. Backwards, the bundle spends its byte
/// budget on the oldest runs and truncates the one being reported. Split out to be testable
/// without a logger.
fn newest_first(current: Vec<PathBuf>, mut rotated: Vec<PathBuf>) -> Vec<PathBuf> {
    rotated.reverse();
    let mut files = current;
    files.extend(rotated);
    files
}

#[cfg(test)]
#[path = "tests/logging_tests.rs"]
mod tests;
