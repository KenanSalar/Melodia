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
    AdaptiveFormat, Age, Cleanup, Criterion, Duplicate, FileSpec, LogfileSelector, Logger,
    LoggerHandle, Naming, WriteMode, detailed_format,
};

use crate::config::Paths;
use crate::error::{AppError, AppResult};

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
const DEFAULT_LOG_SPEC: &str =
    "warn, melodia=info, Melodia=info, symphonia_bundle_mp3::layer3=error";

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

/// Start file logging. Call once, as early in `main` as a [`Paths`] exists.
pub fn install(paths: &Paths) -> AppResult<()> {
    let handle = Logger::try_with_env_or_str(DEFAULT_LOG_SPEC)
        .map_err(AppError::io_source)?
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
        // Adaptive, not plain coloured: env_logger's `auto-color` suppressed
        // ANSI off a tty and piping the app somewhere shouldn't regress.
        .adaptive_format_for_stderr(AdaptiveFormat::Default)
        .start()
        .map_err(AppError::io_source)?;

    let _ = HANDLE.set(handle);
    Ok(())
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
    // Two calls: asked for together, the rotated files come back newest-first
    // but the live one is *appended* after them, so the list is neither
    // chronological nor reverse-chronological and no sort of it is either.
    let mut files = handle
        .existing_log_files(&LogfileSelector::none().with_r_current())
        .unwrap_or_default();
    files.extend(
        handle
            .existing_log_files(&LogfileSelector::default())
            .unwrap_or_default(),
    );
    files
}

#[cfg(test)]
#[path = "tests/logging_tests.rs"]
mod tests;
