//! Moving a station list between Melodia and everything else.
//!
//! **Not `playlist_files`, and not a reach into it.** That module is track-and-database shaped
//! end to end — file paths re-matched by BLAKE3, `#EXTINF` durations, a playlist row created per
//! import — and it knows only Extended-M3U8. A station list shares the file extension and nothing
//! else: the entries are URLs, there is no library to match them against, and the format most
//! stations arrive in is `.pls`.
//!
//! **Not `player::source::stream_source::first_stream_url` either**, though the overlap looks total. That
//! one answers "which single URL is behind this pointer" against a byte-capped live response on
//! the playback path, and deliberately carries no names. This one wants every entry *and* what it
//! is called, off a file the user picked.
//!
//! Export writes Extended-M3U8, the one format every player reads. Import takes `.m3u`, `.m3u8`
//! and `.pls`, because that is what the user will have been handed.

use std::path::Path;

use crate::database::{DbPool, queries};
use crate::entities::radio;
use crate::error::AppError;
use crate::state::AppState;

const HEADER: &str = "#EXTM3U";
const EXTINF_TAG: &str = "#EXTINF:";

/// The four things about a station that are the user's rather than the directory's or the
/// stream's, so a list survives the round trip with their own edits on it.
///
/// **Comments, in `playlist_files`'s `#MELODIA-HASH:` shape and for its reason.** Every player
/// skips a `#` line it does not know, and so does [`parse`] below, so a file written by this build
/// still imports into an older one and into everything else — it simply arrives without them,
/// which is what a file that never had them arrives as anyway. Same of [`STATION_TAG`].
const WEBSITE_TAG: &str = "#MELODIA-WEBSITE:";
const LOGO_TAG: &str = "#MELODIA-LOGO:";
const GENRE_TAG: &str = "#MELODIA-GENRE:";
const COUNTRY_TAG: &str = "#MELODIA-COUNTRY:";

/// The row as the directory or the probe described it, so an import restores what a station *is*
/// rather than a name and a URL — `station_uuid` above all, which is what separates a station the
/// directory owns from a hand-typed lookalike wearing the same title.
///
/// **One JSON blob rather than a tag per column.** [`radio::NewRadioStation`] already carries the
/// set and already derives serde, so a new column travels for free and cannot fall out of step
/// with a parallel tag table. The four above stay tags because they are the half a user hand-edits
/// and the half that must never be spelled twice for one station.
const STATION_TAG: &str = "#MELODIA-STATION:";

/// What an import did, for the toast that reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportStationsResult {
    /// Entries the kept list gained, which is what the toast calls "added" — a station the import
    /// had to star again counts, whether or not its row already existed. The Favorites tab is what
    /// the user checks against.
    pub imported: u32,
    /// Entries already starred, so the import had nothing to do for them. Skipped rather than
    /// refused, so re-importing a file the user has grown since is the obvious thing to do.
    pub skipped: u32,
}

/// One entry of a station playlist: the URL, whatever the file called it, and — if the file is one
/// of ours — what the station was and what the user recorded about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationEntry {
    pub name: Option<String>,
    pub url: String,
    /// All `None` from any file that is not a Melodia export, which is most of them.
    pub overrides: radio::StationOverrides,
    /// `None` from the same files, and from one of ours whose blob was hand-edited past reading.
    pub snapshot: Option<radio::NewRadioStation>,
}

impl StationEntry {
    /// The save input this entry describes, which is what decides the kind it arrives as.
    ///
    /// **The name and the URL still come off their own lines**, which outrank the blob beside
    /// them: those two are what every other player reads and what a hand-edit changes.
    fn to_new_station(&self) -> radio::NewRadioStation {
        let mut station = self.snapshot.clone().unwrap_or_default();
        let from_blob = std::mem::take(&mut station.name);
        station.name = super::radio::resolve_station_name(
            self.name.as_deref().unwrap_or_default(),
            Some(&from_blob),
            &self.url,
        );
        station.stream_url.clone_from(&self.url);
        // `Some("")` is the shape a hand-edited blob arrives in, and `SQLite` reads it as a value
        // rather than a gap under `UNIQUE` — every station carrying one would upsert onto a single
        // row. The guard `DirectoryStation::is_usable` already makes on the directory's side.
        station.station_uuid = station.station_uuid.filter(|uuid| !uuid.is_empty());
        station
    }
}

/// The tags read since the last URL line, waiting for the entry that claims them.
///
/// Grouped so the two push sites in [`parse`] hand over everything pending in one move rather than
/// each remembering the current list of fields — a fifth tag is otherwise a fifth chance to leak
/// one station's value onto the next.
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

/// Write every kept station to one Extended-M3U8 file.
///
/// Returns how many were written. `#EXTINF:-1` throughout — a live stream has no duration, and
/// `-1` is the tag's own spelling for that.
pub async fn export_stations(state: &AppState, dest: &Path) -> Result<u32, AppError> {
    write_station_list(&state.db, dest).await
}

/// [`export_stations`]'s body, narrowed to what it actually reaches so the tests can drive it off
/// a bare pool. `playlist_files`'s shape: the library door takes the state, the work takes the
/// database.
async fn write_station_list(db: &DbPool, dest: &Path) -> Result<u32, AppError> {
    let stations = queries::radio::get_favorite_stations(db).await?;
    let text = serialize(&stations);
    let path = dest.to_path_buf();

    let written = u32::try_from(stations.len()).unwrap_or(u32::MAX);
    tokio::task::spawn_blocking(move || crate::utils::atomic_file::write_text_sync(&path, &text))
        .await
        .map_err(AppError::io_source)??;
    Ok(written)
}

/// Read a station playlist and put everything in it back in the kept list.
///
/// **A merge rather than an insert, which is what makes an export a backup.** `is_listed` keeps a
/// played *directory* station's row alive after its star goes, so a row an entry lands on may be
/// one Recently Played is holding for it — putting the star back is what an import is for.
/// [`super::radio::add_custom_station`] gives the same answer at the other door.
///
/// Deliberately **not** probed: a file of fifty stations would mean fifty connects, and the user
/// asked to import a list rather than to audition one. A dead entry reports at the click, like a
/// directory station that went off air.
pub async fn import_stations_from_file(
    state: &AppState,
    src: &Path,
) -> Result<ImportStationsResult, AppError> {
    read_station_list(&state.db, src).await
}

/// [`import_stations_from_file`]'s body, narrowed for the reason [`write_station_list`] is.
async fn read_station_list(db: &DbPool, src: &Path) -> Result<ImportStationsResult, AppError> {
    let path = src.to_path_buf();
    let body = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path))
        .await
        .map_err(AppError::io_source)??;

    // **One transaction for the whole file.** Every entry was its own implicit commit on a write
    // pool that holds a single connection, so a list of fifty stations queued a couple of hundred
    // of them behind whatever else wanted to write. It also makes the lookup inside `import_one`
    // able to see the rows earlier entries just wrote, which off the read pool it could not: a
    // file naming one station twice used to depend on each write having already committed.
    //
    // All-or-nothing on an error, where before a failure part way left what it had already
    // written. That is the better half of the trade — the errors reachable here are the database
    // being unwritable, which is not a condition the next entry recovers from — and it is the same
    // argument `queries::artwork::repoint_all` makes for its own pass.
    let mut tx = db.write().begin().await?;
    let mut result = ImportStationsResult::default();
    for entry in parse(&body) {
        if import_one(&mut tx, &entry).await? {
            result.imported = result.imported.saturating_add(1);
        } else {
            result.skipped = result.skipped.saturating_add(1);
        }
    }
    tx.commit().await?;
    Ok(result)
}

/// Put one entry in the kept list, answering whether that changed anything.
///
/// **A row already here takes nothing from the file but its star.** A snapshot out of a file is
/// not evidence against a live row, and `set_local_fields` writes all four columns in one
/// statement, so a file naming one of them would clear the three it says nothing about. Nothing is
/// lost by that: a station the user deleted arrives as a new row, and un-starring one never
/// touched its `local_*` columns. It also keeps every row [`apply_overrides`] writes to logo-less,
/// which is the state [`super::radio::heal_station_logo`] needs to reach one.
async fn import_one(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    entry: &StationEntry,
) -> Result<bool, AppError> {
    let station = entry.to_new_station();
    // A file is the one door into the table that never passed the directory, so
    // without this a blocked station is one export away from a row. Counted as
    // skipped rather than reported: the caller has no vocabulary for the difference
    // and giving it one would describe the blocklist to whoever read the toast.
    if crate::services::net::radio_blocklist::blocks(&station) {
        return Ok(false);
    }
    let existing = queries::radio::kept_station_matching(
        &mut **tx,
        station.station_uuid.as_deref(),
        &station.stream_url,
    )
    .await?;

    let Some((id, is_favorite)) = existing else {
        let id = queries::radio::save_station_on(&mut **tx, &station).await?;
        queries::radio::set_favorite_on(&mut **tx, id, true).await?;
        apply_overrides(tx, id, &entry.overrides).await;
        return Ok(true);
    };

    if is_favorite {
        return Ok(false);
    }
    queries::radio::set_favorite_on(&mut **tx, id, true).await?;
    Ok(true)
}

/// Write the four fields the file carried, if it carried any.
///
/// Validated and written rather than put through `radio::set_station_overrides`, which would
/// download a logo per entry — the fifty connects the import already refuses to spend on probing.
/// The repair behind the completion toast fetches them in batches instead, and every row this
/// touches is one the import just created, so all of them qualify.
///
/// Best-effort: one hand-edited line is not worth refusing a file of fifty stations over, and
/// every field is editable on the card.
async fn apply_overrides(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: i64,
    overrides: &radio::StationOverrides,
) {
    if *overrides == radio::StationOverrides::default() {
        return;
    }
    let stored = match super::radio::validated_overrides(overrides) {
        Ok(stored) => stored,
        Err(e) => {
            log::debug!("radio: imported details refused: {}", crate::error::describe(&e));
            return;
        }
    };
    if let Err(e) = queries::radio::set_local_fields_on(&mut **tx, id, &stored).await {
        log::debug!("radio: imported details not stored: {}", crate::error::describe(&e));
    }
}

/// The four `#MELODIA-*:` tags and the field each carries, **read and written off this one
/// table**.
///
/// It used to be two — a list of `&mut` slots for the reader, a list of `local_*` reads for the
/// writer — so the "a fifth is a row in the table" this file already claimed was a fifth row in
/// two tables, either of which could be the one that forgets it. `radio::OverrideField` exists to
/// give the two halves a single name per field.
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

/// Read a `#MELODIA-STATION:` line into `pending`, answering whether the line was one.
///
/// A blob that will not parse is dropped rather than failing the file: everything it carries is a
/// refinement of the name and URL lines the entry already has, so the station still arrives — as a
/// hand-typed one, which is what a list from anywhere else arrives as anyway.
fn take_station_tag(line: &str, pending: &mut Option<radio::NewRadioStation>) -> bool {
    let Some(rest) = line.strip_prefix(STATION_TAG) else {
        return false;
    };
    match serde_json::from_str(rest.trim()) {
        Ok(station) => *pending = Some(station),
        Err(e) => log::debug!("radio: import dropped an unreadable station tag: {e}"),
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
    // Most of a station's line count is its station block, the tags and the URL being short.
    let mut out = String::with_capacity(HEADER.len() + stations.len() * 384);
    out.push_str(HEADER);
    out.push('\n');
    for station in stations {
        out.push_str(EXTINF_TAG);
        out.push_str("-1,");
        out.push_str(&one_line(&station.name));
        out.push('\n');
        // Cannot fail for a struct of strings and numbers, and a station is not worth refusing an
        // export over if it somehow did.
        if let Ok(snapshot) = serde_json::to_string(&station.to_new_station()) {
            out.push_str(STATION_TAG);
            out.push_str(&snapshot);
            out.push('\n');
        }
        // The user's own columns, and only ever from the `local_*` half — which is what
        // `local_override` answers and `website()` and its siblings deliberately do not: the block
        // above is the directory's account of the station and this is the user's answer to what it
        // left blank, so folding the resolved value into either would spell one station out of both.
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
    // Every tag sits between the `#EXTINF:` and the URL that [`serialize`] writes them between, so
    // the entry the URL opens is the one that takes them.
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
                if crate::services::net::is_http_url(value) {
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

        if crate::services::net::is_http_url(line) {
            entries.push(pending.claim(line.to_owned()));
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

#[cfg(test)]
#[path = "tests/radio_files_tests.rs"]
mod tests;
