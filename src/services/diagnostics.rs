//! The hand-over bundle: one text file a reporter attaches to an issue.
//!
//! Everything here is chosen twice — once for being useful in a bug report,
//! once for being safe to publish. **The settings block is an allowlist, never
//! a whole-struct dump**, so a field added to `SettingsData` cannot silently
//! start shipping; `scrobble_credentials.json` is not read by this module at
//! all, at any point.
//!
//! **The log tail rides on a property worth keeping**: no `log::` call site in
//! this tree interpolates a token, session key or password. Anything added that
//! does breaks this file's safety without touching it, so log the fact that a
//! request was signed rather than what signed it.
//!
//! Every path goes through [`redact_home`], since a home directory usually
//! holds a real name.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Local};

use crate::config::Paths;
use crate::database::queries;
use crate::error::{AppError, AppResult};
use crate::services::crash_report::FILE_TS_FORMAT;
use crate::services::settings::read_settings;
use crate::services::{crash_report, logging, redact_home};
use crate::state::AppState;

/// How many recent crash reports the bundle embeds. A panic that reproduces is
/// worth two samples; past that the older ones describe a version the reporter
/// has already updated away from.
const CRASH_REPORTS_IN_BUNDLE: usize = 3;

/// Per-report cap. A backtrace this long has already said what it knows.
const MAX_CRASH_REPORT_BYTES: u64 = 16 * 1024;

/// Total cap across all log files. Enough to hold the session that failed plus
/// the tail of the one before it, and small enough to attach to an issue.
const MAX_LOG_TAIL_BYTES: u64 = 256 * 1024;

/// What to pre-fill the save dialog with. Stamped local, like a crash report's
/// name and for the same reason — a reporter matching a file against "it broke
/// around 2pm" shouldn't have to apply an offset.
pub fn suggested_file_name(now: DateTime<Local>) -> String {
    format!("melodia-diagnostics-{}.txt", now.format(FILE_TS_FORMAT))
}

/// Build the report. Returns the text; the caller owns where it lands.
pub async fn build_report(state: &AppState) -> AppResult<String> {
    let track_count = queries::track::count_tracks(&state.db).await?;
    let folders = queries::folder::get_all_folders(&state.db).await?;
    let enabled_folders = folders.iter().filter(|folder| folder.is_enabled).count();
    let paths = Arc::clone(&state.paths);

    // The rest is file I/O — settings, crash reports, and up to a quarter
    // megabyte of log tail.
    tokio::task::spawn_blocking(move || {
        assemble(&paths, track_count, folders.len(), enabled_folders)
    })
    .await
    .map_err(AppError::io_source)
}

fn assemble(
    paths: &Paths,
    track_count: i64,
    folder_count: usize,
    enabled_folders: usize,
) -> String {
    format!(
        "Melodia diagnostics report\n\
         {facts}\
         \nlibrary\n\
         tracks    : {track_count}\n\
         folders   : {folder_count} ({enabled_folders} enabled)\n\
         {settings}{crashes}{logs}",
        facts = crash_report::system_facts(Local::now()),
        settings = settings_block(paths),
        crashes = crash_block(&paths.logs_dir),
        logs = log_block(),
    )
}

/// The allowlist. Adding a line here is a deliberate act; adding a field to
/// `SettingsData` is not, and must stay that way.
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
         autoupdate: {autoupdate}\n",
        theme = settings.theme_id,
        variant = settings.theme_variant,
        locale = settings.locale,
        titlebar = if settings.window.use_native_titlebar {
            "native"
        } else {
            "custom"
        },
        tray = settings.tray.tray_enabled,
        close_to_tray = settings.tray.close_to_tray,
        crossfade = settings.crossfade.crossfade_enabled,
        equalizer = settings.equalizer.eq_enabled,
        replaygain = settings.replaygain.rg_enabled,
        autoupdate = settings.updates.auto_check_enabled,
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
            let body = tail_of(path, MAX_CRASH_REPORT_BYTES)
                .unwrap_or_else(|| "<unreadable>\n".to_owned());
            format!("\n--- {} ---\n{body}", display_path(path))
        })
        .collect();
    format!("\ncrash reports\n{}", bodies.concat())
}

fn log_block() -> String {
    let files = logging::log_files();
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

    // A seek into the middle of the file lands mid-line; drop the partial one
    // rather than emit a fragment that reads like a whole record.
    let trimmed = if start > 0 {
        text.find('\n').map_or("", |i| &text[i + 1..])
    } else {
        text.as_ref()
    };

    let mut owned = redact_home(trimmed).into_owned();
    if !owned.ends_with('\n') {
        owned.push('\n');
    }
    Some(owned)
}

fn display_path(path: &Path) -> String {
    redact_home(&path.display().to_string()).into_owned()
}

#[cfg(test)]
#[path = "tests/diagnostics_tests.rs"]
mod tests;
