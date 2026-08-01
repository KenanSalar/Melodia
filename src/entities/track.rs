use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Queue + `ViewModel` shape: the minimal fields the player and now-playing
/// bar need to render and the queue-persistence file needs to round-trip.
/// `~10 fields vs Track's 41` cuts clone/serialize cost ~75 %. Use this when
/// you only need playback-relevant identity (id, path, title, artwork) plus
/// the favorite flag — never as a query result for a list view.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct TrackSummary {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    pub duration_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<i32>,
    /// Last playback position — used for queue restore on startup.
    #[serde(default)]
    pub last_position: i64,
    #[serde(default)]
    pub is_favorite: bool,
    /// 0–5 star rating (0 = unrated). Carried on the playback projection so
    /// the Now-Playing controls can display/edit the current track's rating
    /// without an async fetch, and the queue round-trip preserves it.
    #[serde(default)]
    pub rating: i32,
    // ReplayGain — carried here (not just on the full `Track`) so the sync
    // playback path can bake per-track gain into the audio source without an
    // async DB fetch. Stored as `f32` (the DB column is REAL/f64) since the DSP
    // math is `f32` anyway. All `None` for untagged tracks → unity gain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaygain_track_gain: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaygain_track_peak: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaygain_album_gain: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaygain_album_peak: Option<f32>,
}

impl TrackSummary {
    /// Bundle the four `ReplayGain` columns into the playback transport newtype.
    #[must_use]
    pub fn replaygain(&self) -> crate::player::replaygain::TrackReplayGain {
        crate::player::replaygain::TrackReplayGain {
            track_gain: self.replaygain_track_gain,
            track_peak: self.replaygain_track_peak,
            album_gain: self.replaygain_album_gain,
            album_peak: self.replaygain_album_peak,
        }
    }
}

/// Narrow a DB `REAL`/`f64` `ReplayGain` value to the `f32` the DSP uses. The
/// values are tiny (dB gains and 0..~1 peaks), so the narrowing is sub-audible.
#[allow(
    clippy::cast_possible_truncation,
    reason = "ReplayGain dB/peak values are tiny and well within f32 range; the narrowing is intentional"
)]
fn rg_f64_to_f32(v: Option<f64>) -> Option<f32> {
    v.map(|x| x as f32)
}

impl From<Track> for TrackSummary {
    fn from(t: Track) -> Self {
        Self {
            id: t.id,
            file_path: t.file_path,
            file_name: t.file_name,
            title: t.title,
            artist: t.artist,
            album: t.album,
            duration_ms: t.duration_ms,
            artwork_path: t.artwork_path,
            track_number: t.track_number,
            disc_number: t.disc_number,
            last_position: t.last_position,
            is_favorite: t.is_favorite,
            rating: t.rating,
            replaygain_track_gain: rg_f64_to_f32(t.replaygain_track_gain),
            replaygain_track_peak: rg_f64_to_f32(t.replaygain_track_peak),
            replaygain_album_gain: rg_f64_to_f32(t.replaygain_album_gain),
            replaygain_album_peak: rg_f64_to_f32(t.replaygain_album_peak),
        }
    }
}

impl From<&Track> for TrackSummary {
    fn from(t: &Track) -> Self {
        Self {
            id: t.id,
            file_path: t.file_path.clone(),
            file_name: t.file_name.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration_ms: t.duration_ms,
            artwork_path: t.artwork_path.clone(),
            track_number: t.track_number,
            disc_number: t.disc_number,
            last_position: t.last_position,
            is_favorite: t.is_favorite,
            rating: t.rating,
            replaygain_track_gain: rg_f64_to_f32(t.replaygain_track_gain),
            replaygain_track_peak: rg_f64_to_f32(t.replaygain_track_peak),
            replaygain_album_gain: rg_f64_to_f32(t.replaygain_album_gain),
            replaygain_album_peak: rg_f64_to_f32(t.replaygain_album_peak),
        }
    }
}

/// List-view shape: the columns shown in the Tracks table (and the same
/// shape returned by per-album / per-artist / per-genre track queries).
/// `~14 fields vs Track's 41` reduces serialization cost ~65 % for the most
/// common queries. Includes the foreign-key ids (`album_id`, `artist_id`,
/// `genre_id`) so the UI can navigate from a row without a second fetch.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct TrackListRow {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    pub duration_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_path: Option<String>,
    pub is_favorite: bool,
    /// 0–5 star rating (0 = unrated). Surfaced in list rows for the
    /// hover-revealed star control.
    pub rating: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre_id: Option<i64>,
    pub date_added: String,
    #[serde(skip_serializing)]
    pub sort_key: Option<String>,
}

/// The explicit SELECT columns for `TrackSummary` queries.
/// Listed in `TrackSummary`'s field order for legibility — sqlx's derived
/// `FromRow` matches by column name, so the order isn't load-bearing. Used
/// by `get_track_summaries_by_ids` to project only the columns the queue /
/// now-playing-bar / playback paths actually read, instead of reading all
/// 40 columns and discarding 32 of them.
pub const TRACK_SUMMARY_COLUMNS: &[&str] = &[
    "id", "file_path", "file_name", "title", "artist", "album", "duration_ms",
    "artwork_path", "track_number", "disc_number", "last_position", "is_favorite",
    "rating", "replaygain_track_gain", "replaygain_track_peak", "replaygain_album_gain",
    "replaygain_album_peak",
];

/// Comma-separated form of `TRACK_SUMMARY_COLUMNS` for direct `SELECT` usage.
/// Built once on first access and reused (same `OnceLock` pattern as
/// `track_list_columns`).
pub fn track_summary_columns() -> &'static str {
    use std::sync::OnceLock;
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(|| TRACK_SUMMARY_COLUMNS.join(", "))
}

/// The explicit SELECT columns for `TrackListRow` queries.
/// Used by `track.rs` queries (bare) and `playlist.rs` (with table alias via `track_list_columns_prefixed`).
pub const TRACK_LIST_COLUMNS: &[&str] = &[
    "id", "file_path", "file_name", "title", "artist", "album_artist", "album", "genre",
    "track_number", "disc_number", "year", "duration_ms", "artwork_path", "is_favorite",
    "rating", "album_id", "artist_id", "genre_id", "date_added", "sort_key",
];

/// Comma-separated form of `TRACK_LIST_COLUMNS` for direct `SELECT` usage.
/// Built once on first access and reused — every track-list query reads from
/// the same `&'static str`, no re-allocation per call.
pub fn track_list_columns() -> &'static str {
    use std::sync::OnceLock;
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(|| TRACK_LIST_COLUMNS.join(", "))
}

/// Comma-separated form of `TRACK_LIST_COLUMNS` with a table alias prefix
/// (e.g. `"t"` → `"t.id, t.file_path, ..."`). Cached per alias so playlist /
/// join queries don't rebuild the string on every load.
pub fn track_list_columns_prefixed(alias: &str) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<&'static str, &'static str>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // The set of aliases is fixed and small (currently just "t"), so leaking
    // the prefixed string + interning the alias key is bounded.
    if let Some(&hit) = guard.get(alias) {
        return hit;
    }
    let s: &'static str = Box::leak(
        TRACK_LIST_COLUMNS
            .iter()
            .map(|c| format!("{alias}.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
            .into_boxed_str(),
    );
    let key: &'static str = Box::leak(alias.to_owned().into_boxed_str());
    guard.insert(key, s);
    s
}

/// Playlist-export projection: just the fields an Extended-M3U8 line needs
/// (`#EXTINF` duration + "artist - title", `#MELODIA-HASH`, and the path).
/// Reading these five columns instead of a full `Track` avoids ~35 unused
/// column decodes per row when writing a playlist file. `file_hash` is
/// nullable (tracks before retroactive hashing); the writer omits the hash
/// line when it is `None`.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct PlaylistExportRow {
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_hash: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    pub duration_ms: i64,
}

/// The explicit SELECT columns for `PlaylistExportRow` queries.
pub const PLAYLIST_EXPORT_COLUMNS: &[&str] =
    &["file_path", "file_hash", "title", "artist", "duration_ms"];

/// Comma-separated form of `PLAYLIST_EXPORT_COLUMNS` with a table-alias
/// prefix (the export query joins `tracks t` to `playlist_items`). Cached
/// per alias, same pattern as `track_list_columns_prefixed`.
pub fn playlist_export_columns_prefixed(alias: &str) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<&'static str, &'static str>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&hit) = guard.get(alias) {
        return hit;
    }
    let s: &'static str = Box::leak(
        PLAYLIST_EXPORT_COLUMNS
            .iter()
            .map(|c| format!("{alias}.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
            .into_boxed_str(),
    );
    let key: &'static str = Box::leak(alias.to_owned().into_boxed_str());
    guard.insert(key, s);
    s
}

/// Technical-metadata projection for the full-screen Now Playing view's
/// chip row — just the 8 columns `TrackMetaRow` renders. Reading these
/// instead of a full `Track` saves ~33 unused column decodes (most of them
/// `Option<String>`) per track change. See "track projections by use case"
/// in CLAUDE.md.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct TrackMeta {
    pub id: i64,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
}

/// The explicit SELECT columns for `TrackMeta` queries. Listed in
/// `TrackMeta`'s field order for legibility — sqlx's derived `FromRow`
/// matches by column name, so the order isn't load-bearing.
pub const TRACK_META_COLUMNS: &[&str] = &[
    "id", "codec", "bitrate", "sample_rate", "bit_depth", "channels", "year", "genre",
];

/// Comma-separated form of `TRACK_META_COLUMNS` for direct `SELECT` usage.
/// Built once on first access and reused (same `OnceLock` pattern as
/// `track_summary_columns`).
pub fn track_meta_columns() -> &'static str {
    use std::sync::OnceLock;
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(|| TRACK_META_COLUMNS.join(", "))
}

/// Projection backing the Edit-Track-Information dialog: the editable tag
/// columns plus the read-only technical columns the Summary tab shows.
/// Every field is a native `tracks` column (artist/album/genre are stored
/// denormalized as text alongside their FK ids), so the fetch needs no joins.
/// `bpm` is the `REAL` column, hence `f64`.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct TagEditRow {
    pub id: i64,
    pub file_path: String,
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub original_year: Option<i32>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub bpm: Option<f64>,
    pub artwork_path: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub file_size: Option<i64>,
    pub date_modified: Option<String>,
    pub duration_ms: i64,
    pub file_hash: Option<String>,
}

/// The explicit SELECT columns for `TagEditRow` queries. Listed in
/// `TagEditRow`'s field order for legibility — sqlx's derived `FromRow`
/// matches by column name, so the order isn't load-bearing.
pub const TRACK_TAG_EDIT_COLUMNS: &[&str] = &[
    "id", "file_path", "title", "artist", "album_artist", "album", "genre", "year",
    "original_year", "track_number", "disc_number", "composer", "comment", "bpm",
    "artwork_path", "codec", "bitrate", "sample_rate", "bit_depth", "channels",
    "file_size", "date_modified", "duration_ms", "file_hash",
];

/// Comma-separated form of `TRACK_TAG_EDIT_COLUMNS` for direct `SELECT` usage.
/// Built once on first access and reused (same `OnceLock` pattern as
/// `track_summary_columns`).
pub fn track_tag_edit_columns() -> &'static str {
    use std::sync::OnceLock;
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(|| TRACK_TAG_EDIT_COLUMNS.join(", "))
}

/// The metadata a scrobble needs, projected straight off `tracks` at track
/// start. Every field is a native column (artist/album are denormalized
/// text), so the fetch needs no joins. `title` is the track name;
/// the two `musicbrainz_*` ids map to a `ScrobbleTrack`'s recording/release
/// MBIDs. `ScrobbleTrack::from_row` does the conversion.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct ScrobbleRow {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration_ms: i64,
    pub track_number: Option<i32>,
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
}

/// Explicit SELECT columns for `ScrobbleRow` queries, in field order for
/// legibility (sqlx `FromRow` matches by name, so order isn't load-bearing).
pub const SCROBBLE_ROW_COLUMNS: &[&str] = &[
    "id", "title", "artist", "album", "album_artist", "duration_ms", "track_number",
    "musicbrainz_track_id", "musicbrainz_release_id",
];

/// Comma-separated form of `SCROBBLE_ROW_COLUMNS` for direct `SELECT` usage,
/// built once and reused (same `OnceLock` pattern as `track_summary_columns`).
pub fn scrobble_row_columns() -> &'static str {
    use std::sync::OnceLock;
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(|| SCROBBLE_ROW_COLUMNS.join(", "))
}

/// Full database entity: every column on the `tracks` table including
/// playback stats, hashes, and per-track `ReplayGain`. Use this when you need
/// the complete row (track-detail page, scan-time updates, hash backfill);
/// otherwise prefer `TrackListRow` (list views), `TrackSummary` (queue /
/// `ViewModel`), or `TrackMeta` (Now Playing chips) — all are far cheaper to
/// clone and serialize.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_hash: Option<String>,

    // Core metadata
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,

    // Extended metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub musicbrainz_track_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub musicbrainz_release_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_year: Option<i32>,

    // ReplayGain
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaygain_track_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaygain_track_peak: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaygain_album_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaygain_album_peak: Option<f64>,

    // Technical info
    pub duration_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<i32>,

    // Artwork
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_path: Option<String>,

    // Playback state
    pub play_count: i32,
    pub skip_count: i32,
    pub rating: i32,
    pub is_favorite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    pub last_position: i64,

    // Relations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre_id: Option<i64>,
    pub folder_id: i64,

    // Timestamps
    pub date_added: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_modified: Option<String>,

    // Precomputed natural sort key
    #[serde(skip_serializing)]
    pub sort_key: Option<String>,
}

/// Aggregate stats for the Favorites hero header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FavoriteStats {
    pub count: i64,
    pub total_duration_ms: i64,
    pub artwork_paths: Vec<String>,
}

/// Lightweight struct for "Most Played Favorites" cards.
///
/// `Hash` is what the Favorites grid compares against its last applied set —
/// derived rather than hand-listed so a new field can't silently drop out of
/// the comparison and leave a card stale.
#[derive(Clone, Debug, PartialEq, Hash, FromRow, Serialize, Deserialize)]
pub struct MostPlayedFavorite {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub artwork_path: Option<String>,
    pub play_count: i32,
    pub duration_ms: i64,
}

#[cfg(test)]
#[path = "tests/track_tests.rs"]
mod tests;
