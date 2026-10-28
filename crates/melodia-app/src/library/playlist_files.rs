//! Playlist backup to Extended-M3U8 files, and back into a fresh library.
//!
//! One playlist exports as a bare `.m3u8`, several as one zip holding a `.m3u8` each. Never several
//! in one `.m3u8`: the format carries one `#PLAYLIST:` per file, so every player, ours included,
//! would read them as one merged list.
//!
//! Import is **skip-and-report**: each entry re-matches by exact `file_path`, then by BLAKE3
//! `file_hash`, and the rest count as missing, the user being expected to have rescanned first.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDateTime};

use melodia_core::entities::smart_criteria::SmartCriteria;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::atomic_file;
use melodia_store::database::{DbPool, queries};

mod archive;
mod m3u;

/// What a playlist is exported as.
const PLAYLIST_EXTENSION: &str = "m3u8";

/// What a single playlist file is read back from, the export's own extension included.
pub const PLAYLIST_EXTENSIONS: [&str; 2] = [PLAYLIST_EXTENSION, "m3u"];

/// What several playlists export as together.
pub const ARCHIVE_EXTENSION: &str = "zip";

/// In chars, well under the common 255-byte filename limit even for multi-byte names.
const MAX_STEM_CHARS: usize = 120;

/// Illegal as a stem even with an extension: `CON.m3u8` still resolves to the device.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug)]
pub struct ExportPlaylistsResult {
    pub exported: u32,
    /// Ticked playlists that couldn't be read, left out so the rest still export.
    pub failed: u32,
}

#[derive(Debug)]
pub enum ExportOutcome {
    Exported(ExportPlaylistsResult),
    /// Nothing was written: together the playlists hold more text than an import reads back.
    TooLarge,
}

#[derive(Debug)]
pub struct ImportPlaylistResult {
    pub playlist_id: i64,
    pub playlist_name: String,
    pub total_entries: u32,
    pub matched_by_path: u32,
    pub matched_by_hash: u32,
    pub missing: u32,
}

/// What picked files added. A playlist file adds one playlist, an archive as many as it holds.
#[derive(Debug, Default)]
pub struct ImportFileResult {
    pub imported: u32,
    /// Entries matched to a library track, by path or by hash.
    pub matched: u32,
    pub missing: u32,
    /// Files, or playlists inside an archive, that couldn't be imported, skipped so the rest
    /// still land.
    pub failed: u32,
}

impl ImportFileResult {
    fn add(&mut self, playlist: &ImportPlaylistResult) {
        self.imported = self.imported.saturating_add(1);
        self.matched = self
            .matched
            .saturating_add(playlist.matched_by_path)
            .saturating_add(playlist.matched_by_hash);
        self.missing = self.missing.saturating_add(playlist.missing);
    }

    fn merge(&mut self, file: &Self) {
        self.imported = self.imported.saturating_add(file.imported);
        self.matched = self.matched.saturating_add(file.matched);
        self.missing = self.missing.saturating_add(file.missing);
        self.failed = self.failed.saturating_add(file.failed);
    }
}

/// Sanitized for the save dialog as much as for the disk: a `/` in the name would read as a folder
/// to save into.
pub fn suggested_file_name(playlist_name: &str) -> String {
    format!("{}.{PLAYLIST_EXTENSION}", sanitize_stem(playlist_name))
}

/// Dated, so a second backup doesn't open on the first one's name.
pub fn suggested_archive_name(now: DateTime<Local>) -> String {
    format!("melodia-playlists-{}.{ARCHIVE_EXTENSION}", now.format("%Y-%m-%d"))
}

/// Write one playlist to exactly `dest`, the save dialog having already asked before overwriting.
pub async fn export_playlist_to_file(
    db: &DbPool,
    playlist_id: i64,
    dest: &Path,
) -> Result<(), AppError> {
    let (_, text) = prepare_export(db, playlist_id).await?;
    atomic_file::write_text(dest.to_path_buf(), text).await
}

/// Write each requested playlist as a `.m3u8` inside one zip at `dest`.
///
/// A playlist deleted since it was ticked is left out and counted rather than failing the batch.
/// Playlists holding more text than [`archive::MAX_TEXT_BYTES`] write nothing at all, import being
/// bound to refuse the archive.
pub async fn export_playlists_to_archive(
    db: &DbPool,
    playlist_ids: &[i64],
    dest: &Path,
) -> Result<ExportOutcome, AppError> {
    write_archive(db, playlist_ids, dest, Local::now().naive_local()).await
}

/// Takes the instant its entries are dated, so a test can pin it.
async fn write_archive(
    db: &DbPool,
    playlist_ids: &[i64],
    dest: &Path,
    modified: NaiveDateTime,
) -> Result<ExportOutcome, AppError> {
    let mut entries = Vec::with_capacity(playlist_ids.len());
    let mut used = HashSet::new();
    let mut budget = archive::TextBudget::default();
    let mut failed: u32 = 0;
    for &id in playlist_ids {
        match prepare_export(db, id).await {
            Ok((name, mut text)) => {
                let Ok(()) = budget.charge(text.len()) else {
                    return Ok(ExportOutcome::TooLarge);
                };
                // Held beside every other playlist's until the archive is written.
                text.shrink_to_fit();
                entries.push(archive::Entry { name: unique_entry_name(&name, &mut used), text });
            }
            Err(e) => {
                log::warn!("playlist export: left out playlist {id}: {}", describe(&e));
                failed = failed.saturating_add(1);
            }
        }
    }

    let exported = u32::try_from(entries.len()).unwrap_or(u32::MAX);
    if !entries.is_empty() {
        let path = dest.to_path_buf();
        tokio::task::spawn_blocking(move || {
            atomic_file::write_with_sync(&path, |out| archive::write(out, &entries, modified))
        })
        .await
        .map_err(AppError::io_source)??;
    }

    Ok(ExportOutcome::Exported(ExportPlaylistsResult { exported, failed }))
}

/// A playlist's name and its M3U8 text. A smart playlist writes the tracks its rules match now and
/// imports back as a regular one, M3U having nowhere to keep the rules.
async fn prepare_export(db: &DbPool, playlist_id: i64) -> Result<(String, String), AppError> {
    let stats = queries::playlist::get_playlist_by_id(db, playlist_id).await?;
    let tracks = if stats.is_smart {
        let criteria = SmartCriteria::from_json_opt(stats.smart_criteria.as_deref());
        queries::smart_playlist::get_smart_playlist_tracks_for_export(db, &criteria).await?
    } else {
        queries::playlist::get_playlist_tracks_for_export(db, playlist_id).await?
    };
    let text = m3u::serialize(&stats.name, &tracks);
    Ok((stats.name, text))
}

/// Import every playlist in each of `paths`, each into a new playlist. A file that fails whole
/// counts once, so the files beside it still land.
pub async fn import_playlists_from_files(db: &DbPool, paths: &[PathBuf]) -> ImportFileResult {
    let mut total = ImportFileResult::default();
    for path in paths {
        match import_playlists_from_file(db, path).await {
            Ok(file) => total.merge(&file),
            Err(e) => {
                log::warn!("playlist import: {}: {}", path.display(), describe(&e));
                total.failed = total.failed.saturating_add(1);
            }
        }
    }
    total
}

/// Import the playlist a playlist file holds, or each one in an archive.
///
/// A playlist with no entries is refused rather than created empty from a garbage file; one whose
/// entries match no library track still lands and reports the misses. Within an archive a refusal
/// is one skipped playlist, and only an archive that won't open, holds no playlist files or expands
/// past [`archive::MAX_TEXT_BYTES`] fails whole.
async fn import_playlists_from_file(db: &DbPool, src: &Path) -> Result<ImportFileResult, AppError> {
    if has_extension(src, &[ARCHIVE_EXTENSION]) {
        return read_archive(db, src).await;
    }
    let mut result = ImportFileResult::default();
    result.add(&read_playlist_file(db, src).await?);
    Ok(result)
}

async fn read_playlist_file(db: &DbPool, src: &Path) -> Result<ImportPlaylistResult, AppError> {
    let content = tokio::fs::read_to_string(src).await?;
    import_text(db, &content, src).await
}

async fn read_archive(db: &DbPool, src: &Path) -> Result<ImportFileResult, AppError> {
    let path = src.to_path_buf();
    let contents = tokio::task::spawn_blocking(move || archive::read(std::fs::File::open(&path)?))
        .await
        .map_err(AppError::io_source)??;
    if contents.entries.is_empty() && contents.unreadable == 0 {
        return Err(AppError::validation_msg("No playlist files found in archive"));
    }

    let beside = src.parent().unwrap_or_else(|| Path::new(""));
    let mut result =
        ImportFileResult { failed: contents.unreadable, ..ImportFileResult::default() };
    for entry in contents.entries {
        match import_text(db, &entry.text, &beside.join(&entry.name)).await {
            Ok(playlist) => result.add(&playlist),
            Err(e) => {
                log::warn!(
                    "playlist import: {} in {}: {}",
                    entry.name,
                    src.display(),
                    describe(&e)
                );
                result.failed = result.failed.saturating_add(1);
            }
        }
    }
    Ok(result)
}

/// `origin` is where the file sits, or would once extracted beside its archive: its stem names a
/// playlist with no `#PLAYLIST:` tag, and relative entries resolve against its folder.
async fn import_text(
    db: &DbPool,
    content: &str,
    origin: &Path,
) -> Result<ImportPlaylistResult, AppError> {
    let parsed = m3u::parse(content);

    if parsed.entries.is_empty() {
        return Err(AppError::validation_msg("No playlist entries found in file"));
    }

    let name = parsed
        .playlist_name
        .clone()
        .or_else(|| origin.file_stem().and_then(OsStr::to_str).map(ToOwned::to_owned))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Imported Playlist".to_owned());

    let outcome = match_entries(db, &parsed.entries, origin.parent()).await?;

    let playlist_id =
        queries::playlist::create_playlist_with_tracks(db, &name, None, &outcome.ordered_ids)
            .await?;

    Ok(ImportPlaylistResult {
        playlist_id,
        playlist_name: name,
        total_entries: u32::try_from(parsed.entries.len()).unwrap_or(u32::MAX),
        matched_by_path: outcome.matched_by_path,
        matched_by_hash: outcome.matched_by_hash,
        missing: outcome.missing,
    })
}

struct MatchOutcome {
    /// In file order, misses omitted.
    ordered_ids: Vec<i64>,
    matched_by_path: u32,
    matched_by_hash: u32,
    missing: u32,
}

/// Split from [`import_text`] so matching is testable without creating a playlist.
async fn match_entries(
    db: &DbPool,
    entries: &[m3u::ParsedEntry],
    base: Option<&Path>,
) -> Result<MatchOutcome, AppError> {
    let resolved: Vec<String> = entries.iter().map(|e| resolve_path(&e.path, base)).collect();

    let mut ids: Vec<Option<i64>> = vec![None; entries.len()];
    let mut matched_by_path: u32 = 0;
    let mut matched_by_hash: u32 = 0;

    let by_path = queries::track::get_track_ids_by_paths(db, &resolved).await?;
    for (i, path) in resolved.iter().enumerate() {
        if let Some(&id) = by_path.get(path) {
            ids[i] = Some(id);
            matched_by_path += 1;
        }
    }

    let hashes: Vec<String> = entries
        .iter()
        .enumerate()
        .filter(|(i, _)| ids[*i].is_none())
        .filter_map(|(_, e)| e.hash.clone())
        .collect();
    if !hashes.is_empty() {
        let by_hash = queries::track::get_track_ids_by_hashes(db, &hashes).await?;
        for (i, entry) in entries.iter().enumerate() {
            if ids[i].is_some() {
                continue;
            }
            if let Some(id) = entry.hash.as_deref().and_then(|h| by_hash.get(h)) {
                ids[i] = Some(*id);
                matched_by_hash += 1;
            }
        }
    }

    let missing = u32::try_from(ids.iter().filter(|id| id.is_none()).count()).unwrap_or(u32::MAX);
    let ordered_ids: Vec<i64> = ids.into_iter().flatten().collect();

    Ok(MatchOutcome { ordered_ids, matched_by_path, matched_by_hash, missing })
}

fn resolve_path(entry_path: &str, base: Option<&Path>) -> String {
    let p = Path::new(entry_path);
    if p.is_absolute() {
        entry_path.to_owned()
    } else if let Some(b) = base {
        b.join(p).to_string_lossy().into_owned()
    } else {
        entry_path.to_owned()
    }
}

/// A filename stem safe on every platform. Trailing dots and spaces go because Windows drops them.
fn sanitize_stem(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();

    s = s.trim().trim_end_matches(['.', ' ']).to_owned();
    if s.is_empty() {
        return "playlist".to_owned();
    }

    if WINDOWS_RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&s)) {
        s.insert(0, '_');
    }

    if s.chars().count() > MAX_STEM_CHARS {
        s = s.chars().take(MAX_STEM_CHARS).collect();
        s = s.trim_end_matches(['.', ' ']).to_owned();
        if s.is_empty() {
            return "playlist".to_owned();
        }
    }

    s
}

/// A `.m3u8` entry name no earlier entry holds, compared lowercased since the archive may be
/// extracted onto a filesystem that folds case.
fn unique_entry_name(name: &str, used: &mut HashSet<String>) -> String {
    let stem = sanitize_stem(name);
    let mut candidate = format!("{stem}.{PLAYLIST_EXTENSION}");
    let mut n: u32 = 2;
    while used.contains(&candidate.to_ascii_lowercase()) {
        candidate = format!("{stem} ({n}).{PLAYLIST_EXTENSION}");
        n = n.saturating_add(1);
    }
    used.insert(candidate.to_ascii_lowercase());
    candidate
}

/// Whether `path` ends in one of `extensions`, ignoring case: a picker hands back `.M3U8` as
/// readily as `.m3u8`.
fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| extensions.iter().any(|known| known.eq_ignore_ascii_case(ext)))
}

#[cfg(test)]
#[path = "playlist_files/tests/playlist_files_tests.rs"]
mod tests;
