//! Panic reports, written beside the rolling log.
//!
//! `panic = "abort"` makes every Rust panic fatal, so a hook is complete coverage of the Rust half,
//! and it does run under abort: `rust_panic_with_hook` dispatches the hook before
//! `__rust_start_panic`. A run produces at most one report.
//!
//! **A backtrace here is sparse.** `strip = "debuginfo"` keeps symbols but no line table, and fat
//! LTO inlines hard, so frames carry names without `file:line`. The `log` macros' own `file:line`
//! fills the gap, which is why `logging` formats files with `detailed_format`.
//!
//! Timestamps are **local**, unlike everything else persisted (`utils::now_rfc3339` is UTC): the
//! reports sit beside `flexi_logger`'s local-stamped lines and get read together, and "it crashed
//! around 2pm" should match the filename. The RFC-3339 offset keeps it unambiguous.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Local, NaiveDateTime};

use melodia_core::error::describe;
use melodia_core::utils::redact::redact_home;

/// Bounded by usefulness rather than cost: a reporter is asked for the crash they just had.
const MAX_CRASH_REPORTS: usize = 10;

const PREFIX: &str = "crash-";
const SUFFIX: &str = ".txt";

/// The newest report the user has been shown, so a crash is announced once.
///
/// Retired by neither sweep sharing the directory: it carries neither [`PREFIX`] nor [`SUFFIX`],
/// so [`timestamp_of`] rejects it, and `flexi_logger`'s cleanup matches its `melodia` basename and
/// `log` suffix.
const LAST_SEEN_FILE: &str = "last-seen-crash.json";

/// Sortable, path-safe, second-resolution: the process aborts on its first panic, so two reports
/// collide only across concurrent instances. Shared with `services::diagnostics`, whose bundle is
/// read beside these.
pub const FILE_TS_FORMAT: &str = "%Y%m%d-%H%M%S";

/// Held for one trip through the hook, so two threads panicking at once don't interleave a write,
/// a prune and a `log::error!`.
///
/// **Not a recursion guard, though it reads like one**: a panic inside the hook aborts before
/// dispatch, so only a concurrent panic on another thread loses the exchange, and it still calls
/// `previous`. Cleared on the way out rather than latched, because a dev build unwinds and a tokio
/// task swallows its panic, and a latch would hide every panic after the first.
static IN_HOOK: AtomicBool = AtomicBool::new(false);

struct HookGuard;

impl Drop for HookGuard {
    fn drop(&mut self) {
        IN_HOOK.store(false, Ordering::SeqCst);
    }
}

/// The one definition of the naming scheme; [`timestamp_of`] is its inverse.
fn file_name(now: DateTime<Local>) -> String {
    format!("{PREFIX}{}{SUFFIX}", now.format(FILE_TS_FORMAT))
}

/// When a report was written, or `None` for a name this module didn't write. Pruning is gated on
/// this, so it is the whole of what keeps the sweep off `melodia_rCURRENT.log` beside it.
fn timestamp_of(name: &str) -> Option<NaiveDateTime> {
    parse_stamp(name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?)
}

fn parse_stamp(stamp: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(stamp, FILE_TS_FORMAT).ok()
}

/// Installs the panic hook, as early as a logs directory exists so boot panics are covered.
/// Chains rather than replaces, `panic::update_hook` being unstable, so `cargo run` keeps the
/// default stderr message.
pub fn install_hook(logs_dir: &Path) {
    let logs_dir = logs_dir.to_path_buf();
    let previous = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        if IN_HOOK.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            previous(info);
            return;
        }
        let _guard = HookGuard;

        let now = Local::now();
        let thread = std::thread::current();
        let location = info.location().map(ToString::to_string);
        let payload = info.payload_as_str().unwrap_or("<non-string panic payload>");
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();

        let report = format_report(now, thread.name(), location.as_deref(), payload, &backtrace);

        // Plain `fs`: a panic raised inside a logging call must not need the logger to leave an
        // artifact.
        let path = logs_dir.join(file_name(now));
        let _ = std::fs::create_dir_all(&logs_dir);
        let _ = std::fs::write(&path, report);
        prune(&logs_dir);

        // Only now the logger, so the rolling log closes in sequence with what led here.
        log::error!(
            "panic at {}: {payload} — report written to {}",
            location.as_deref().unwrap_or("<unknown location>"),
            path.display()
        );

        previous(info);
    }));
}

/// Pure over its inputs, so testable without panicking.
fn format_report(
    now: DateTime<Local>,
    thread: Option<&str>,
    location: Option<&str>,
    payload: &str,
    backtrace: &str,
) -> String {
    format!(
        "Melodia crash report\n\
         {facts}\
         thread    : {thread}\n\
         location  : {location}\n\
         panic     : {payload}\n\
         \n\
         backtrace\n\
         {backtrace}\n",
        facts = system_facts(now),
        thread = thread.unwrap_or("<unnamed>"),
        location = location.unwrap_or("<unknown>"),
        payload = redact_home(payload),
        backtrace = redact_home(backtrace),
    )
}

/// The header both this report and the diagnostics bundle open with.
pub fn system_facts(now: DateTime<Local>) -> String {
    // Absent off Linux, where the pair says nothing worth a line.
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
    let session = if session_type.is_none() && desktop.is_none() {
        String::new()
    } else {
        format!(
            "session   : {} / {}\n",
            session_type.as_deref().unwrap_or("unknown"),
            desktop.as_deref().unwrap_or("unknown")
        )
    };

    format!(
        "version   : {version}\n\
         timestamp : {timestamp}\n\
         os / arch : {os} / {arch}\n\
         {session}\
         install   : {install}\n",
        version = env!("CARGO_PKG_VERSION"),
        timestamp = now.to_rfc3339(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        // `None` on macOS and unsupported arches, where the install method is unknown, not absent.
        install = crate::services::platform::install_kind::target::current_target_key()
            .unwrap_or("unknown"),
    )
}

/// The newest report the user hasn't been told about, marking it seen, so the boot toast fires
/// once per crash.
pub fn take_unseen(logs_dir: &Path) -> Option<PathBuf> {
    // `existing_reports` is oldest first.
    let (newest, path) = existing_reports(logs_dir).pop()?;

    let marker = logs_dir.join(LAST_SEEN_FILE);
    let seen: LastSeen =
        melodia_core::utils::atomic_file::load_json_or_default_sync(&marker).unwrap_or_default();
    if seen.newest.as_deref().and_then(parse_stamp) >= Some(newest) {
        return None;
    }

    let record = LastSeen { newest: Some(newest.format(FILE_TS_FORMAT).to_string()) };
    if let Err(e) = melodia_core::utils::atomic_file::write_json_sync(&marker, &record) {
        // Hand the report over anyway: a notice that repeats beats one that never arrives.
        log::warn!("crash report marker: {}", describe(&e));
    }
    Some(path)
}

/// A [`FILE_TS_FORMAT`] string rather than a `NaiveDateTime`, chrono being pulled without its
/// `serde` feature.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct LastSeen {
    #[serde(default)]
    newest: Option<String>,
}

/// Reports newest first, at most `limit`.
pub fn recent(logs_dir: &Path, limit: usize) -> Vec<PathBuf> {
    let mut reports = existing_reports(logs_dir);
    reports.reverse();
    reports.truncate(limit);
    reports.into_iter().map(|(_, path)| path).collect()
}

/// Every report in `logs_dir` that parses back into this module's scheme, oldest first, so nothing
/// else in the directory can be swept.
fn existing_reports(logs_dir: &Path) -> Vec<(NaiveDateTime, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return Vec::new();
    };

    let mut reports: Vec<(NaiveDateTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stamp = timestamp_of(path.file_name()?.to_str()?)?;
            Some((stamp, path))
        })
        .collect();

    reports.sort_unstable_by_key(|(stamp, _)| *stamp);
    reports
}

/// Deletes all but the newest [`MAX_CRASH_REPORTS`].
fn prune(logs_dir: &Path) {
    let reports = existing_reports(logs_dir);
    let Some(surplus) = reports.len().checked_sub(MAX_CRASH_REPORTS) else {
        return;
    };
    for (_, path) in &reports[..surplus] {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
#[path = "tests/crash_report_tests.rs"]
mod tests;
