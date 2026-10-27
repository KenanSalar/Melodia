//! The file log sink.
//!
//! Every run writes `logs/melodia_rCURRENT.log` with no env var set: a `.desktop`, tray or Windows
//! GUI launch leaves no console to capture stderr from, and nobody re-runs under `RUST_LOG` after
//! the crash they already had.
//!
//! # Which level
//!
//! - **`info`**: happened once, matters at a glance. On for every user, so the rotation budget is
//!   sized against its volume.
//! - **`warn`**: a user could notice. Expected, self-recovering and unbounded goes to `debug`
//!   with a count instead; per occurrence it only teaches the reader to skim warnings
//!   (`player::playback::stream_health`).
//! - **`debug`**: what the user did, as a narrative: one line per class of action at the seam
//!   every path funnels through, and **nothing on a timer, frame or keystroke**. The *action*,
//!   never the widget: a play/pause line at the button misses the shortcut, the tray, the media
//!   keys and Now Playing. `trace` is unused, hence [`VERBOSE_LEVEL`] stopping here.
//!
//! One writer, `single_instance` forwarding a second launch before `install` runs. Nothing here
//! defends the file though: an unmakeable claim boots anyway.

use std::path::PathBuf;
use std::sync::OnceLock;

use flexi_logger::{
    AdaptiveFormat, Age, Cleanup, Criterion, Duplicate, FileSpec, FlexiLoggerError,
    LogSpecification, LogfileSelector, Logger, LoggerHandle, Naming, WriteMode, detailed_format,
};

use melodia_core::config::Paths;

/// What our own targets log at with Verbose Logging off.
const NORMAL_LEVEL: &str = "info";

/// Not `trace`: no `log::trace!` site exists, so it would differ only in what the muted
/// dependencies say.
const VERBOSE_LEVEL: &str = "debug";

/// Two dependency modules warning about what we already know: `layer3`'s bit-reservoir underflow
/// fires per *frame* on a stream not opened at byte zero while the decoder plays it fine, and
/// `sctk_adwaita::buttons` names controls on the titlebar winit builds and hides in
/// custom-titlebar mode.
///
/// Scoped by module, never by crate, each being the only `warn!` in its own. Shared by both specs:
/// a verbose spec dropping `layer3` buries the detail the switch was flipped for.
const SPEC_TAIL: &str = "symphonia_bundle_mp3::layer3=error, sctk_adwaita::buttons=error";

/// Dependency warnings, our own narrative at `level`, and nothing else, a bare `"info"` spending
/// the rotation budget on every crate in the graph. `RUST_LOG` overrides it.
///
/// **`melodia` is a prefix, not a target**: directives match by `starts_with`, so one token covers
/// every `melodia_*` member. `Melodia` is the binary, the match being case-sensitive.
fn spec_for(level: &str) -> String {
    format!("warn, melodia={level}, Melodia={level}, {SPEC_TAIL}")
}

/// Rotate at 2 MiB or the turn of the day; `Cleanup` never counts the live file, so the ceiling is
/// 8 files. Counted rather than dated: `KeepForDays` drops the byte bound, and its startup sweep
/// wipes an occasional user's whole history before they can report anything.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LOG_FILES: usize = 7;

/// `flexi_logger` shuts its writers down when the handle drops.
static HANDLE: OnceLock<LoggerHandle> = OnceLock::new();

/// So a diagnostics bundle can tell a broken sink from an install that never wrote logs.
static FILE_SINK_ERROR: OnceLock<String> = OnceLock::new();

/// Whether `RUST_LOG` was **set** at [`install`], which makes [`set_verbose`] decline. Presence,
/// not a parse: a malformed value is still a developer saying which levels they want.
static ENV_SPEC_WINS: OnceLock<bool> = OnceLock::new();

/// Starts logging, as early in `main` as a [`Paths`] exists.
///
/// **Infallible on purpose**: it degrades to stderr-only rather than refusing to start Melodia
/// over a log file. `verbose` is an argument because the flag lives in `settings.json`, a file
/// above this layer, and it has to apply at boot, a boot going wrong being what the switch is for.
pub fn install(paths: &Paths, verbose: bool) {
    let _ = ENV_SPEC_WINS.set(std::env::var_os("RUST_LOG").is_some());

    let spec = spec_for(if verbose { VERBOSE_LEVEL } else { NORMAL_LEVEL });

    let error = match start_to_file(paths, &spec) {
        Ok(handle) => {
            let _ = HANDLE.set(handle);
            return;
        }
        // `OutputIo` never interpolates its `io::Error`, so without the chain a root-owned file
        // and a full disk read identically.
        Err(e) => melodia_core::error::describe(&e),
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

/// Everything the fallback keeps.
fn base_logger(spec: &str) -> Logger {
    // A malformed `RUST_LOG` falls back to `spec`, so only a broken `spec_for` errors. `warn`
    // rather than `default()`, which is `off()` and would cost the process its logger too.
    Logger::try_with_env_or_str(spec)
        .unwrap_or_else(|_| Logger::with(LogSpecification::warn()))
        // Adaptive, so piping the app somewhere doesn't regress.
        .adaptive_format_for_stderr(AdaptiveFormat::Default)
}

fn start_to_file(paths: &Paths, spec: &str) -> Result<LoggerHandle, FlexiLoggerError> {
    base_logger(spec)
        .log_to_file(
            FileSpec::default().directory(&paths.logs_dir).basename("melodia").suppress_timestamp(),
        )
        // Without it a restart truncates, and the run that crashed is gone by the time the folder
        // is opened.
        .append()
        .rotate(
            Criterion::AgeOrSize(Age::Day, MAX_LOG_BYTES),
            Naming::Numbers,
            Cleanup::KeepLogFiles(KEEP_LOG_FILES),
        )
        .cleanup_in_background_thread(false)
        // The default, stated because `process::exit(0)` runs no destructors and a buffered mode
        // drops exactly the lines before a crash.
        .write_mode(WriteMode::Direct)
        .duplicate_to_stderr(Duplicate::All)
        .format_for_files(detailed_format)
        .start()
}

/// Why there are no log files, or `None` when the file sink is running.
pub fn unavailable_reason() -> Option<&'static str> {
    FILE_SINK_ERROR.get().map(String::as_str)
}

/// Swaps the running level on the live sinks. **`RUST_LOG` wins** and this declines out loud: a GUI
/// switch fighting the developer escape hatch would make the variable mean nothing.
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
        Err(e) => log::warn!(
            "verbose logging: could not parse the spec '{spec}': {}",
            melodia_core::error::describe(&e)
        ),
    }
}

/// For an exit path that runs no destructors. A no-op under [`WriteMode::Direct`], and what saves
/// the tail if that ever changes.
pub fn flush() {
    if let Some(handle) = HANDLE.get() {
        handle.flush();
    }
}

/// The current log file and its rotated siblings, newest first. An unreadable directory reads as no
/// logs, a bundle missing a file beating one that failed to build.
pub fn log_files() -> Vec<PathBuf> {
    let Some(handle) = HANDLE.get() else {
        return Vec::new();
    };
    // Two calls: asked together, the live file lands after the rotated ones.
    let current =
        handle.existing_log_files(&LogfileSelector::none().with_r_current()).unwrap_or_default();
    let rotated = handle.existing_log_files(&LogfileSelector::default()).unwrap_or_default();
    newest_first(current, rotated)
}

/// **The reversal is the whole of it**, and `FileSpec` reads as arguing against it:
/// `existing_log_files` ends in a plain `sort()`, so the API hands back ascending names and a
/// higher `Naming::Numbers` index is the *newer* file. Backwards, the bundle spends its budget on
/// the oldest runs and truncates the one being reported.
fn newest_first(current: Vec<PathBuf>, mut rotated: Vec<PathBuf>) -> Vec<PathBuf> {
    rotated.reverse();
    let mut files = current;
    files.extend(rotated);
    files
}

#[cfg(test)]
#[path = "tests/logging_tests.rs"]
mod tests;
