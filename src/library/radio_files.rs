//! Moving a station list between Melodia and everything else.
//!
//! **Not `playlist_files`, and not a reach into it.** That module is track-and-database shaped
//! end to end — file paths re-matched by BLAKE3, `#EXTINF` durations, a playlist row created per
//! import — and it knows only Extended-M3U8. A station list shares the file extension and nothing
//! else: the entries are URLs, there is no library to match them against, and the format most
//! stations arrive in is `.pls`.
//!
//! **Not `player::stream_source::first_stream_url` either**, though the overlap looks total. That
//! one answers "which single URL is behind this pointer" against a byte-capped live response on
//! the playback path, and deliberately carries no names. This one wants every entry *and* what it
//! is called, off a file the user picked.
//!
//! Export writes Extended-M3U8, the one format every player reads. Import takes `.m3u`, `.m3u8`
//! and `.pls`, because that is what the user will have been handed.

use std::path::Path;

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::state::AppState;

const HEADER: &str = "#EXTM3U";
const EXTINF_TAG: &str = "#EXTINF:";

/// What an import did, for the toast that reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportStationsResult {
    pub imported: u32,
    /// Entries whose stream URL was already in the table. Skipped rather than refused, so
    /// re-importing a file the user has grown since is the obvious thing to do.
    pub skipped: u32,
}

/// One entry of a station playlist: the URL, and whatever the file called it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationEntry {
    pub name: Option<String>,
    pub url: String,
}

/// Write every kept station to one Extended-M3U8 file.
///
/// Returns how many were written. `#EXTINF:-1` throughout — a live stream has no duration, and
/// `-1` is the tag's own spelling for that.
pub async fn export_stations(state: &AppState, dest: &Path) -> Result<u32, AppError> {
    let stations = queries::radio::get_favorite_stations(&state.db).await?;
    let text = serialize(&stations);
    let path = dest.to_path_buf();

    let written = u32::try_from(stations.len()).unwrap_or(u32::MAX);
    tokio::task::spawn_blocking(move || crate::services::write_text_atomic_sync(&path, &text))
        .await
        .map_err(AppError::io_source)??;
    Ok(written)
}

/// Read a station playlist and keep everything in it that is not already here.
///
/// Imported stations are hand-typed stations in every respect — no `station_uuid`, starred on the
/// way in — because that is exactly what they are. Deliberately **not** probed: a file of fifty
/// stations would mean fifty connects, and the user asked to import a list rather than to audition
/// one. A dead entry reports at the click, like a directory station that went off air.
pub async fn import_stations_from_file(
    state: &AppState,
    src: &Path,
) -> Result<ImportStationsResult, AppError> {
    let path = src.to_path_buf();
    let body = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path))
        .await
        .map_err(AppError::io_source)??;

    let mut result = ImportStationsResult::default();
    for entry in parse(&body) {
        if queries::radio::station_id_with_url(&state.db, &entry.url).await?.is_some() {
            result.skipped = result.skipped.saturating_add(1);
            continue;
        }
        let station = radio::NewRadioStation {
            name: super::radio::resolve_station_name(
                entry.name.as_deref().unwrap_or_default(),
                None,
                &entry.url,
            ),
            stream_url: entry.url,
            ..Default::default()
        };
        let saved = queries::radio::save_station(&state.db, &station).await?;
        queries::radio::set_favorite(&state.db, saved.id, true).await?;
        result.imported = result.imported.saturating_add(1);
    }
    Ok(result)
}

/// Replace CR/LF with a space, so a name carrying one cannot break the single-line tag format.
fn one_line(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(['\r', '\n']) {
        std::borrow::Cow::Owned(s.replace(['\r', '\n'], " "))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// Stations as Extended-M3U8 text: `\n` endings, trailing newline, UTF-8 with no BOM.
fn serialize(stations: &[radio::RadioStation]) -> String {
    let mut out = String::with_capacity(HEADER.len() + stations.len() * 96);
    out.push_str(HEADER);
    out.push('\n');
    for station in stations {
        out.push_str(EXTINF_TAG);
        out.push_str("-1,");
        out.push_str(&one_line(&station.name));
        out.push('\n');
        out.push_str(&one_line(&station.stream_url));
        out.push('\n');
    }
    out
}

/// Every station a playlist body names, across the two formats a station list arrives in.
///
/// One pass rather than two parsers, on the same reading `stream_source` uses: an `.m3u` carries
/// the URL on a line of its own with `#EXTINF` above it, and a `.pls` carries `FileN=` with an
/// optional `TitleN=` anywhere in the same section. A `key=value` line whose key is not `FileN` or
/// `TitleN` is a `.pls` field we have no use for; anything else that parses as an absolute HTTP
/// URL is an `.m3u` entry.
///
/// `.pls` pairs by index rather than by order — the spec does not require `TitleN` to follow its
/// `FileN`, and real files put every title after every file.
fn parse(body: &str) -> Vec<StationEntry> {
    let mut entries: Vec<StationEntry> = Vec::new();
    // `FileN`'s index against where its entry landed, so a later `TitleN` can find it.
    let mut pls_slots: Vec<(u32, usize)> = Vec::new();
    let mut pending_name: Option<String> = None;

    for line in body.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix(EXTINF_TAG) {
            pending_name = extinf_title(rest);
            continue;
        }
        if line.starts_with('#') || line.starts_with('[') {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            if let Some(index) = indexed_key(key.trim(), "File") {
                if is_http_url(value) {
                    pls_slots.push((index, entries.len()));
                    entries.push(StationEntry {
                        name: pending_name.take(),
                        url: value.to_owned(),
                    });
                }
                continue;
            }
            if let Some(index) = indexed_key(key.trim(), "Title") {
                let slot = pls_slots.iter().find(|(n, _)| *n == index).map(|(_, at)| *at);
                if let Some(at) = slot
                    && let Some(entry) = entries.get_mut(at)
                    && !value.is_empty()
                {
                    entry.name = Some(value.to_owned());
                }
                continue;
            }
            // A bare `.m3u` URL can carry `=` in its query string, so an unrecognised key falls
            // through to the URL reading rather than being swallowed here.
        }

        if is_http_url(line) {
            entries.push(StationEntry {
                name: pending_name.take(),
                url: line.to_owned(),
            });
        }
    }
    entries
}

/// The display title out of an `#EXTINF:` payload — everything past the first comma.
///
/// The *first*, so a title carrying commas survives round-tripping through [`serialize`]. It also
/// means an HLS-style attribute list (`-1 tvg-name="A,B",Title`) truncates at the attribute's own
/// comma — not worth a parser for, station lists being the one dialect that doesn't use them.
fn extinf_title(payload: &str) -> Option<String> {
    let (_duration, title) = payload.split_once(',')?;
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// The `N` of a `.pls` `<prefix>N` key, case-insensitively.
fn indexed_key(key: &str, prefix: &str) -> Option<u32> {
    let (head, index) = key.split_at_checked(prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    index.parse().ok()
}

/// Whether `value` is an absolute `http`/`https` URL. The scheme check is the whole filter: a
/// `file://` line in a playlist somebody sent you is not a station.
fn is_http_url(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    lowered.starts_with("http://") || lowered.starts_with("https://")
}

#[cfg(test)]
#[path = "tests/radio_files_tests.rs"]
mod tests;
