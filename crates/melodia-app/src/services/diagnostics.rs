//! The bug-report bundle: one text file a reporter attaches to a public issue, so each part is
//! chosen for being safe to publish as well as useful.
//!
//! **The settings block is an allowlist, never a whole-struct dump**, so a new `SettingsData`
//! field can't start shipping, and `scrobble_credentials.json` is never read here. The log tail is
//! safe only while no `log::` call writes a credential (root `CLAUDE.md`), and every path goes
//! through [`redact_home`], a home directory usually holding a real name.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Local};

use crate::services::settings::read_settings;
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult, describe};
use melodia_core::utils::redact::redact_home;
use melodia_platform::services::platform::crash_report::FILE_TS_FORMAT;
use melodia_platform::services::platform::{crash_report, logging};
use melodia_store::database::queries;

/// A reproducing panic is worth two samples; older ones describe a version since updated away.
const CRASH_REPORTS_IN_BUNDLE: usize = 3;

/// Per report, spent from the front ([`head_of`]): a backtrace this long has said what it knows.
const MAX_CRASH_REPORT_BYTES: u64 = 16 * 1024;

/// Across all log files: the session that failed plus the tail of the one before it, and still
/// small enough to attach to an issue.
const MAX_LOG_TAIL_BYTES: u64 = 256 * 1024;

/// The save dialog's pre-filled name, in local time like everything else the trio writes.
pub fn suggested_file_name(now: DateTime<Local>) -> String {
    format!("melodia-diagnostics-{}.txt", now.format(FILE_TS_FORMAT))
}

/// The report's text; where it lands is the caller's.
pub async fn build_report(state: &AppState) -> AppResult<String> {
    let library = library_facts(state).await;
    let paths = Arc::clone(&state.paths);

    tokio::task::spawn_blocking(move || assemble(&paths, &library))
        .await
        .map_err(AppError::io_source)
}

/// What the database could answer, each fact degrading on its own: a broken database is a likely
/// thing to be reporting, and must not cost the reporter the logs and crash reports.
struct LibraryFacts {
    tracks: Option<i64>,
    folders: Option<FolderCounts>,
}

struct FolderCounts {
    total: usize,
    enabled: usize,
}

async fn library_facts(state: &AppState) -> LibraryFacts {
    LibraryFacts {
        tracks: queries::track::count_tracks(&state.db)
            .await
            .inspect_err(|e| log::warn!("diagnostics: track count unavailable: {}", describe(e)))
            .ok(),
        folders: queries::folder::get_all_folders(&state.db)
            .await
            .inspect_err(|e| log::warn!("diagnostics: folder list unavailable: {}", describe(e)))
            .ok()
            .map(|folders| FolderCounts {
                total: folders.len(),
                enabled: folders.iter().filter(|folder| folder.is_enabled).count(),
            }),
    }
}

fn assemble(paths: &Paths, library: &LibraryFacts) -> String {
    format!(
        "Melodia diagnostics report\n\
         {facts}\
         data dir  : {data_dir}\n\
         {library}{settings}{crashes}{logs}",
        facts = crash_report::system_facts(Local::now()),
        // Not left to the boot log, whose tail budget drops it from the long sessions worth
        // reporting, nor to `system_facts`, which has no `Paths`.
        data_dir = display_path(&paths.data_dir),
        library = library_block(library),
        settings = settings_block(paths),
        crashes = crash_block(&paths.logs_dir),
        logs = log_block(),
    )
}

fn library_block(facts: &LibraryFacts) -> String {
    format!(
        "\nlibrary\n\
         tracks    : {tracks}\n\
         folders   : {folders}\n",
        tracks = facts.tracks.map_or_else(|| "<unavailable>".to_owned(), |count| count.to_string()),
        folders = facts.folders.as_ref().map_or_else(
            || "<unavailable>".to_owned(),
            |counts| format!("{} ({} enabled)", counts.total, counts.enabled),
        ),
    )
}

/// The allowlist: a line here is a deliberate act, where a `SettingsData` field is not.
fn settings_block(paths: &Paths) -> String {
    let Ok(settings) = read_settings(paths) else {
        return "\nsettings\n<unreadable>\n".to_owned();
    };

    format!(
        "\nsettings\n\
         theme     : {theme} / {variant}\n\
         locale    : {locale}\n\
         titlebar  : {titlebar}\n\
         tray      : {tray} (close-to-tray {close_to_tray})\n\
         crossfade : {crossfade}\n\
         equalizer : {equalizer}\n\
         replaygain: {replaygain}\n\
         autoupdate: {autoupdate}\n\
         verbose   : {verbose}\n",
        theme = settings.theme_id,
        variant = settings.theme_variant,
        locale = settings.locale,
        titlebar = if settings.window.use_native_titlebar { "native" } else { "custom" },
        tray = settings.tray.tray_enabled,
        close_to_tray = settings.tray.close_to_tray,
        crossfade = settings.crossfade.crossfade_enabled,
        equalizer = settings.equalizer.eq_enabled,
        replaygain = settings.replaygain.rg_enabled,
        autoupdate = settings.updates.auto_check_enabled,
        // Says whether a thin log tail means a quiet session or detail nobody asked for.
        verbose = settings.diagnostics.verbose_logging,
    )
}

fn crash_block(logs_dir: &Path) -> String {
    let reports = crash_report::recent(logs_dir, CRASH_REPORTS_IN_BUNDLE);
    if reports.is_empty() {
        return "\ncrash reports\n<none>\n".to_owned();
    }

    let bodies: Vec<String> = reports
        .iter()
        .map(|path| {
            let body = head_of(path, MAX_CRASH_REPORT_BYTES)
                .unwrap_or_else(|| "<unreadable>\n".to_owned());
            format!("\n--- {} ---\n{body}", display_path(path))
        })
        .collect();
    format!("\ncrash reports\n{}", bodies.concat())
}

fn log_block() -> String {
    log_section(logging::unavailable_reason(), logging::log_files())
}

/// Split from [`log_block`] so both kinds of empty are reachable without a live logger.
fn log_section(unavailable: Option<&str>, files: Vec<PathBuf>) -> String {
    if let Some(reason) = unavailable {
        // Not `<none>`: a sink that couldn't start wants another answer than no log written.
        return format!("\nlogs\n<unavailable: {}>\n", redact_home(reason));
    }

    if files.is_empty() {
        return "\nlogs\n<none>\n".to_owned();
    }

    // Newest first, so the budget is spent on what happened most recently.
    let mut sections = Vec::with_capacity(files.len());
    let mut budget = MAX_LOG_TAIL_BYTES;
    for path in files {
        if budget == 0 {
            break;
        }
        let Some(text) = tail_of(&path, budget) else {
            continue;
        };
        budget = budget.saturating_sub(u64::try_from(text.len()).unwrap_or(u64::MAX));
        sections.push(format!("\n--- {} ---\n{text}", display_path(&path)));
    }
    format!("\nlogs\n{}", sections.concat())
}

/// The last `max_bytes` of `path`, starting at a line boundary.
fn tail_of(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut buf = Vec::with_capacity(usize::try_from(len - start).unwrap_or(0));
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);

    // A seek lands mid-line; drop the fragment rather than pass it off as a record.
    let trimmed =
        if start > 0 { text.find('\n').map_or("", |i| &text[i + 1..]) } else { text.as_ref() };

    Some(redacted_lines(trimmed))
}

/// The first `max_bytes` of `path`, ending at a line boundary. A crash report opens with what a
/// reporter is asked for and runs long only in its backtrace, so it keeps its head where a log
/// keeps its tail.
fn head_of(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let mut buf = Vec::with_capacity(usize::try_from(len.min(max_bytes)).unwrap_or(0));
    file.by_ref().take(max_bytes).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);

    // Only after a cut: a whole file without a trailing newline keeps its last line.
    let trimmed =
        if len > max_bytes { text.rfind('\n').map_or("", |i| &text[..=i]) } else { text.as_ref() };

    Some(redacted_lines(trimmed))
}

/// Both readers' exit: home redacted, and a closing newline so the next header starts a line.
fn redacted_lines(text: &str) -> String {
    let mut owned = redact_home(text).into_owned();
    if !owned.ends_with('\n') {
        owned.push('\n');
    }
    owned
}

fn display_path(path: &Path) -> String {
    redact_home(&path.display().to_string()).into_owned()
}

#[cfg(test)]
#[path = "tests/diagnostics_tests.rs"]
mod tests;
