//! Moving a station list between Melodia and everything else. Export writes Extended-M3U8, the one
//! format every player reads; import takes `.m3u`, `.m3u8` and `.pls`.
//!
//! Not `playlist_files`, which is track-and-database shaped (BLAKE3 re-matching, a playlist row per
//! import), and not `stream_source::first_stream_url`, which wants one URL off a live response and
//! no names.
//!
//! **The one module besides `library::radio` that may name `queries::radio`**: an import writes
//! station rows without passing the facade.

use std::path::{Path, PathBuf};

use melodia_core::entities::radio;
use melodia_core::error::{AppError, describe};
use melodia_store::database::{DbPool, queries};

const HEADER: &str = "#EXTM3U";
const EXTINF_TAG: &str = "#EXTINF:";

/// The four fields that are the user's rather than the directory's or the stream's, so a list
/// survives the round trip with their edits on it. Comments, in `playlist_files`' `#MELODIA-HASH:`
/// shape: every player, [`parse`] included, skips a `#` line it doesn't know, so an older build or
/// another player imports the file without them. Same of [`STATION_TAG`].
const WEBSITE_TAG: &str = "#MELODIA-WEBSITE:";
const LOGO_TAG: &str = "#MELODIA-LOGO:";
const GENRE_TAG: &str = "#MELODIA-GENRE:";
const COUNTRY_TAG: &str = "#MELODIA-COUNTRY:";

/// The row as the directory or the probe described it, `station_uuid` above all, which separates a
/// directory station from a hand-typed lookalike with the same title.
///
/// **One JSON blob of [`radio::NewRadioStation`] rather than a tag per column**, so a new column
/// travels for free. The four above stay tags as the half a user hand-edits.
const STATION_TAG: &str = "#MELODIA-STATION:";

/// What an import did, for the toast that reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportStationsResult {
    /// Entries the kept list gained, a station starred again counting whether or not its row
    /// existed: the Favorites tab is what the user checks against.
    pub imported: u32,
    /// Entries already starred. Skipped rather than refused, so a file the user has grown since
    /// re-imports cleanly.
    pub skipped: u32,
}

/// [`ImportStationsResult`] summed over several files, beside the files that failed whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportStationFiles {
    pub stations: ImportStationsResult,
    pub failed: u32,
}

/// One entry of a station playlist: the URL, whatever the file called it, and — if the file is one
/// of ours — what the station was and what the user recorded about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationEntry {
    pub name: Option<String>,
    pub url: String,
    /// All `None` from any file that is not a Melodia export.
    pub overrides: radio::StationOverrides,
    /// `None` from the same files, and from one of ours whose blob was hand-edited past reading.
    pub snapshot: Option<radio::NewRadioStation>,
}

impl StationEntry {
    /// The name and URL lines outrank the blob: they are what every other player reads and what a
    /// hand-edit changes.
    fn to_new_station(&self) -> radio::NewRadioStation {
        let mut station = self.snapshot.clone().unwrap_or_default();
        let from_blob = std::mem::take(&mut station.name);
        station.name = super::radio::resolve_station_name(
            self.name.as_deref().unwrap_or_default(),
            Some(&from_blob),
            &self.url,
        );
        station.stream_url.clone_from(&self.url);
        // `UNIQUE` reads a hand-edited `Some("")` as a value, so every station carrying one would
        // upsert onto a single row; `DirectoryStation::is_usable` guards the directory's side.
        station.station_uuid = station.station_uuid.filter(|uuid| !uuid.is_empty());
        station
    }

    /// `None` for a station the blocklist refuses, a file being the one door into the table that
    /// never passed the directory. The caller counts it as skipped, since saying more would
    /// describe the blocklist to whoever reads the toast.
    fn to_import(&self) -> Option<queries::radio::ImportedStation> {
        let station = self.to_new_station();
        if melodia_net::services::net::radio_blocklist::blocks(&station) {
            return None;
        }
        Some(queries::radio::ImportedStation {
            station,
            overrides: stored_overrides(&self.overrides),
        })
    }
}

/// The tags read since the last URL line, grouped so both push sites in [`parse`] hand over
/// everything in one move and a new tag can't leak one station's value onto the next.
#[derive(Default)]
struct Pending {
    name: Option<String>,
    overrides: radio::StationOverrides,
    snapshot: Option<radio::NewRadioStation>,
}

impl Pending {
    /// Hand everything read so far to the entry `url` opens, leaving nothing for the next.
    fn claim(&mut self, url: String) -> StationEntry {
        StationEntry {
            name: self.name.take(),
            url,
            overrides: std::mem::take(&mut self.overrides),
            snapshot: self.snapshot.take(),
        }
    }
}

/// Write every kept station to one Extended-M3U8 file, returning how many. `#EXTINF:-1`
/// throughout, the tag's own spelling for a live stream's missing duration.
pub async fn export_stations(db: &DbPool, dest: &Path) -> Result<u32, AppError> {
    let stations = queries::radio::get_favorite_stations(db).await?;
    let text = serialize(&stations);

    let written = u32::try_from(stations.len()).unwrap_or(u32::MAX);
    melodia_core::utils::atomic_file::write_text(dest.to_path_buf(), text).await?;
    Ok(written)
}

/// [`import_stations_from_file`] over each of `paths`, totalled for the completion toast. A file
/// that fails whole counts once in `failed`, so the files beside it still land.
pub async fn import_stations_from_files(db: &DbPool, paths: &[PathBuf]) -> ImportStationFiles {
    let mut total = ImportStationFiles::default();
    for path in paths {
        match import_stations_from_file(db, path).await {
            Ok(file) => {
                total.stations.imported = total.stations.imported.saturating_add(file.imported);
                total.stations.skipped = total.stations.skipped.saturating_add(file.skipped);
            }
            Err(e) => {
                total.failed = total.failed.saturating_add(1);
                log::warn!("radio: import {}: {}", path.display(), describe(&e));
            }
        }
    }
    total
}

/// Read a station playlist back into the kept list.
///
/// **A merge rather than an insert, which is what makes an export a backup**: `is_listed` keeps a
/// played directory station's row after its star goes, so an entry can land on a row Recently
/// Played holds, and putting the star back is the point. **Not probed**: fifty stations would be
/// fifty connects, and a dead entry reports at the click like a station gone off air.
pub async fn import_stations_from_file(
    db: &DbPool,
    src: &Path,
) -> Result<ImportStationsResult, AppError> {
    let path = src.to_path_buf();
    let body = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path))
        .await
        .map_err(AppError::io_source)??;

    let entries = parse(&body);
    let stations: Vec<_> = entries.iter().filter_map(StationEntry::to_import).collect();
    let imported = queries::radio::import_stations(db, &stations).await?;
    let listed = u32::try_from(entries.len()).unwrap_or(u32::MAX);
    Ok(ImportStationsResult { imported, skipped: listed.saturating_sub(imported) })
}

/// The file's four fields validated for storing, or `None` with nothing to write.
///
/// Not through `radio::set_station_overrides`, which downloads a logo per entry; the repair behind
/// the completion toast fetches them in batches. A field that won't validate drops the entry's
/// overrides rather than the entry, every one being editable on the card.
fn stored_overrides(overrides: &radio::StationOverrides) -> Option<radio::StationOverrides> {
    if *overrides == radio::StationOverrides::default() {
        return None;
    }
    super::radio::validated_overrides(overrides)
        .inspect_err(|e| {
            log::debug!("radio: imported details refused: {}", melodia_core::error::describe(e));
        })
        .ok()
}

/// The four `#MELODIA-*:` tags and their fields, **read and written off this one table**, so a
/// fifth is one row.
const OVERRIDE_TAGS: [(&str, radio::OverrideField); 4] = [
    (WEBSITE_TAG, radio::OverrideField::Website),
    (LOGO_TAG, radio::OverrideField::LogoUrl),
    (GENRE_TAG, radio::OverrideField::Genre),
    (COUNTRY_TAG, radio::OverrideField::Country),
];

/// Read one `#MELODIA-*:` line into `pending`, answering whether the line was one.
fn take_override_tag(line: &str, pending: &mut radio::StationOverrides) -> bool {
    for (tag, field) in OVERRIDE_TAGS {
        if let Some(rest) = line.strip_prefix(tag) {
            let rest = rest.trim();
            *pending.slot_mut(field) = (!rest.is_empty()).then(|| rest.to_owned());
            return true;
        }
    }
    false
}

/// Read a `#MELODIA-STATION:` line into `pending`, answering whether the line was one. A blob that
/// won't parse is dropped, not fatal: the station still arrives off its name and URL lines, as a
/// hand-typed one.
fn take_station_tag(line: &str, pending: &mut Option<radio::NewRadioStation>) -> bool {
    let Some(rest) = line.strip_prefix(STATION_TAG) else {
        return false;
    };
    match serde_json::from_str(rest.trim()) {
        Ok(station) => *pending = Some(station),
        Err(e) => log::debug!(
            "radio: import dropped an unreadable station tag: {}",
            melodia_core::error::describe(&e)
        ),
    }
    true
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
    // Mostly the station block; the tags and the URL are short.
    let mut out = String::with_capacity(HEADER.len() + stations.len() * 384);
    out.push_str(HEADER);
    out.push('\n');
    for station in stations {
        out.push_str(EXTINF_TAG);
        out.push_str("-1,");
        out.push_str(&one_line(&station.name));
        out.push('\n');
        // Can't fail for strings and numbers, and one station isn't worth refusing the export over.
        if let Ok(snapshot) = serde_json::to_string(&station.to_new_station()) {
            out.push_str(STATION_TAG);
            out.push_str(&snapshot);
            out.push('\n');
        }
        // Only the `local_*` half, which `local_override` answers and `website()` and its siblings
        // don't: the block above is the directory's account, these the user's answer to what it
        // left blank, and a resolved value would spell one station out of both.
        for (tag, field) in OVERRIDE_TAGS {
            if let Some(value) = station.local_override(field).filter(|text| !text.is_empty()) {
                out.push_str(tag);
                out.push_str(&one_line(value));
                out.push('\n');
            }
        }
        out.push_str(&one_line(&station.stream_url));
        out.push('\n');
    }
    out
}

/// Every station a playlist body names, `.m3u` and `.pls` in one pass, read as `stream_source` reads
/// them. `.pls` pairs `TitleN` with `FileN` by index, real files putting every title after every
/// file.
fn parse(body: &str) -> Vec<StationEntry> {
    let mut entries: Vec<StationEntry> = Vec::new();
    // `FileN`'s index against where its entry landed, for a later `TitleN`.
    let mut pls_slots: Vec<(u32, usize)> = Vec::new();
    // [`serialize`] writes every tag between the `#EXTINF:` and its URL, so the URL's entry takes
    // them.
    let mut pending = Pending::default();

    for line in body.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix(EXTINF_TAG) {
            pending.name = extinf_title(rest);
            continue;
        }
        if take_override_tag(line, &mut pending.overrides) {
            continue;
        }
        if take_station_tag(line, &mut pending.snapshot) {
            continue;
        }
        if line.starts_with('#') || line.starts_with('[') {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            if let Some(index) = indexed_key(key.trim(), "File") {
                if melodia_net::services::net::is_http_url(value) {
                    pls_slots.push((index, entries.len()));
                    entries.push(pending.claim(value.to_owned()));
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

        if melodia_net::services::net::is_http_url(line) {
            entries.push(pending.claim(line.to_owned()));
        }
    }
    entries
}

/// Everything past the *first* comma, so a title with commas round-trips through [`serialize`]. An
/// HLS attribute list (`tvg-name="A,B"`) truncates at its own comma, which station lists don't use.
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

#[cfg(test)]
#[path = "tests/radio_files_tests.rs"]
mod tests;
