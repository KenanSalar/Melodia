//! Playlist import/export to Extended-M3U8 files.
//!
//! Lets a user back up playlists to portable `.m3u8` files (one per playlist)
//! so they survive a database wipe (OS reinstall), and import them back into
//! a fresh library. Interoperates with other players: our writer emits
//! standard Extended-M3U8 plus a `#MELODIA-HASH` extension, and our parser
//! tolerantly reads foreign M3U/M3U8 files.
//!
//! Import is **skip-and-report**: each entry is re-matched to a library track
//! by exact `file_path`, then by BLAKE3 `file_hash`; anything unmatched is
//! counted as *missing* (the user is expected to have re-scanned their music
//! first). Pure format handling lives in [`m3u`].

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;

use crate::database::{DbPool, queries};
use crate::error::AppError;
use crate::state::AppState;

mod m3u;

/// Maximum sanitized stem length (chars) before the `.m3u8` extension, well
/// under common 255-byte filename limits even for multi-byte names.
const MAX_STEM_CHARS: usize = 120;

/// Windows reserved device names — illegal as a filename stem even with an
/// extension (`CON.m3u8` still resolves to the device), so we prefix them.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Clone, Debug, Serialize)]
pub struct ExportPlaylistsResult {
    pub exported: u32,
    /// Display path of the destination folder (for the completion toast).
    pub folder: String,
    /// `(playlist name, error message)` for playlists that failed to write.
    pub failed: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportPlaylistResult {
    pub playlist_id: i64,
    pub playlist_name: String,
    pub total_entries: u32,
    pub matched_by_path: u32,
    pub matched_by_hash: u32,
    pub missing: u32,
}

/// Write each requested playlist to `<sanitized-name>.m3u8` inside `folder`.
///
/// Per-playlist failures (missing playlist, write error) are collected into
/// the result's `failed` list rather than aborting the whole batch. Filenames
/// are sanitized and de-duplicated both within the batch and against files
/// already in `folder` (` (2)`, ` (3)`, … suffixes).
pub async fn export_playlists_to_folder(
    state: &AppState,
    playlist_ids: &[i64],
    folder: &Path,
) -> Result<ExportPlaylistsResult, AppError> {
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut prepared: Vec<(String, String)> = Vec::with_capacity(playlist_ids.len());

    for &id in playlist_ids {
        match prepare_export(&state.db, id).await {
            Ok((name, text)) => prepared.push((name, text)),
            Err(e) => failed.push((format!("playlist {id}"), e.to_string())),
        }
    }

    // Resolve collision-free filenames (sync, cheap) before writing.
    let mut used: HashSet<String> = HashSet::new();
    let mut to_write: Vec<(std::path::PathBuf, String)> = Vec::with_capacity(prepared.len());
    for (name, text) in prepared {
        let filename = unique_filename(folder, &name, &mut used);
        to_write.push((folder.join(filename), text));
    }

    let (exported, write_errs) = tokio::task::spawn_blocking(move || {
        let mut ok: u32 = 0;
        let mut errs: Vec<(String, String)> = Vec::new();
        for (path, text) in to_write {
            match crate::utils::atomic_file::write_text_sync(&path, &text) {
                Ok(()) => ok += 1,
                Err(e) => {
                    let label =
                        path.file_name().and_then(|f| f.to_str()).unwrap_or("playlist").to_owned();
                    errs.push((label, e.to_string()));
                }
            }
        }
        (ok, errs)
    })
    .await
    .map_err(AppError::io_source)?;

    failed.extend(write_errs);

    Ok(ExportPlaylistsResult {
        exported,
        folder: folder.display().to_string(),
        failed,
    })
}

/// Fetch a playlist's name + ordered tracks and render them to M3U8 text.
async fn prepare_export(db: &DbPool, playlist_id: i64) -> Result<(String, String), AppError> {
    let stats = queries::playlist::get_playlist_by_id(db, playlist_id).await?;
    let tracks = queries::playlist::get_playlist_tracks_for_export(db, playlist_id).await?;
    let text = m3u::serialize(&stats.name, &tracks);
    Ok((stats.name, text))
}

/// Parse `src`, create a NEW playlist (named from `#PLAYLIST:` or the file
/// stem), re-match every entry to a library track, and add the resolved ids
/// in file order. Returns per-category counts for the completion toast.
///
/// Errors only when the file has no entries at all (so we never create a
/// phantom empty playlist from a garbage file). A file whose entries simply
/// don't match any library track still creates the playlist and reports the
/// misses.
pub async fn import_playlist_from_file(
    state: &AppState,
    src: &Path,
) -> Result<ImportPlaylistResult, AppError> {
    let content = tokio::fs::read_to_string(src).await?;
    let parsed = m3u::parse(&content);

    if parsed.entries.is_empty() {
        return Err(AppError::Validation("No playlist entries found in file".to_owned()));
    }

    let name = parsed
        .playlist_name
        .clone()
        .or_else(|| src.file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Imported Playlist".to_owned());

    let outcome = match_entries(&state.db, &parsed.entries, src.parent()).await?;

    let playlist = queries::playlist::create_playlist(&state.db, &name, None).await?;
    queries::playlist::add_tracks_to_playlist(&state.db, playlist.id, &outcome.ordered_ids).await?;

    Ok(ImportPlaylistResult {
        playlist_id: playlist.id,
        playlist_name: name,
        total_entries: u32::try_from(parsed.entries.len()).unwrap_or(u32::MAX),
        matched_by_path: outcome.matched_by_path,
        matched_by_hash: outcome.matched_by_hash,
        missing: outcome.missing,
    })
}

/// Result of re-matching parsed entries to library track ids.
struct MatchOutcome {
    /// Resolved track ids in file order (misses omitted).
    ordered_ids: Vec<i64>,
    matched_by_path: u32,
    matched_by_hash: u32,
    missing: u32,
}

/// Re-match parsed entries to library tracks: pass 1 exact `file_path`, pass 2
/// BLAKE3 `file_hash`. Relative entry paths are resolved against `base` (the
/// playlist file's directory) before path matching.
///
/// Factored out of [`import_playlist_from_file`] so it can be tested against a
/// bare [`DbPool`] without constructing a full [`AppState`].
async fn match_entries(
    db: &DbPool,
    entries: &[m3u::ParsedEntry],
    base: Option<&Path>,
) -> Result<MatchOutcome, AppError> {
    let resolved: Vec<String> = entries.iter().map(|e| resolve_path(&e.path, base)).collect();

    let mut ids: Vec<Option<i64>> = vec![None; entries.len()];
    let mut matched_by_path: u32 = 0;
    let mut matched_by_hash: u32 = 0;

    // Pass 1 — exact path.
    let by_path = queries::track::get_track_ids_by_paths(db, &resolved).await?;
    for (i, path) in resolved.iter().enumerate() {
        if let Some(&id) = by_path.get(path) {
            ids[i] = Some(id);
            matched_by_path += 1;
        }
    }

    // Pass 2 — BLAKE3 hash, for still-unmatched entries that carry one.
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

    Ok(MatchOutcome {
        ordered_ids,
        matched_by_path,
        matched_by_hash,
        missing,
    })
}

/// Resolve a (possibly relative) entry path to a string for path matching.
/// Absolute paths pass through; relative paths join `base` (the playlist
/// file's directory) when available.
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

/// Sanitize a playlist name into a cross-platform-safe filename stem (no
/// extension). Replaces illegal/control characters with `_`, strips trailing
/// dots/spaces (Windows drops them), guards reserved device names, caps the
/// length, and falls back to `"playlist"` when nothing usable remains.
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

/// A collision-free `.m3u8` filename within `folder`. Tracks chosen names in
/// `used` (lowercased) and also checks the filesystem, appending ` (2)`,
/// ` (3)`, … before the extension on collision.
fn unique_filename(folder: &Path, name: &str, used: &mut HashSet<String>) -> String {
    let stem = sanitize_stem(name);
    let mut candidate = format!("{stem}.m3u8");
    let mut n: u32 = 2;
    while used.contains(&candidate.to_ascii_lowercase()) || folder.join(&candidate).exists() {
        candidate = format!("{stem} ({n}).m3u8");
        n = n.saturating_add(1);
    }
    used.insert(candidate.to_ascii_lowercase());
    candidate
}

#[cfg(test)]
#[path = "playlist_files/tests/playlist_files_tests.rs"]
mod tests;
