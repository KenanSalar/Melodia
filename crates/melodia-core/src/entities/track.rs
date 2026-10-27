//! Track rows, and the projections queries select into through `queries::track::columns`. A list
//! fetching the whole [`Track`] pays a decode per unused column per row, so take the slimmest that
//! carries what you render, and the closest existing one over a near-duplicate:
//!
//! | projection | for |
//! |---|---|
//! | [`TrackSummary`] | queue, Now Playing, the sync playback path (carries `ReplayGain` and rating) |
//! | [`TrackListRow`] | every track list, Browse included |
//! | [`TrackMeta`] | the Now-Playing technical chips |
//! | [`TagEditRow`] | the Edit-Track-Information dialog |
//! | [`ScrobbleRow`] | a scrobble or love submission |
//! | [`PlaylistExportRow`] | one M3U8 line |
//! | [`TrackLinks`] | the three FK ids a "Go to …" menu entry navigates by |
//!
//! A library-sized list holds no projection per track, only ids (`ui::paged_rows`).

use sqlx::FromRow;

/// What the player and the bar render, cloned into every published view model, so it carries
/// playback identity and nothing a list draws.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct TrackSummary {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: i64,
    pub artwork_path: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    /// Last playback position — used for queue restore on startup.
    pub last_position: i64,
    pub is_favorite: bool,
    /// 0–5, 0 unrated; here so Now Playing edits it without a fetch.
    pub rating: i32,
    // Here so the sync playback path bakes the gain without a fetch; `f32` as the DSP is, and all
    // `None` is unity.
    pub replaygain_track_gain: Option<f32>,
    pub replaygain_track_peak: Option<f32>,
    pub replaygain_album_gain: Option<f32>,
    pub replaygain_album_peak: Option<f32>,
}

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

/// Every track list's row. The FK ids let a row navigate without a second fetch; no list draws
/// the path, so Browse, which matches rows to files on disk, fetches it beside.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct TrackListRow {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub duration_ms: i64,
    pub artwork_path: Option<String>,
    pub is_favorite: bool,
    /// 0–5, 0 unrated.
    pub rating: i32,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub genre_id: Option<i64>,
    pub sort_key: Option<String>,
}

/// One Extended-M3U8 line. `file_hash` is `None` before the retroactive hash, and the writer then
/// omits that line.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct PlaylistExportRow {
    pub file_path: String,
    pub file_hash: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub duration_ms: i64,
}

/// Now Playing's technical chips.
#[derive(Clone, Debug, PartialEq, FromRow)]
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

/// The Edit Track Information dialog, native columns only. The multi-valued fields are rows it
/// reads apart, an editor fed from a rendered line splitting `Chanson, Francaise` in two
/// (`.claude/rules/library-data.md`).
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct TagEditRow {
    pub id: i64,
    pub file_path: String,
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub original_year: Option<i32>,
    pub track_number: Option<i32>,
    pub track_total: Option<i32>,
    pub disc_number: Option<i32>,
    pub disc_total: Option<i32>,
    pub disc_subtitle: Option<String>,
    pub subtitle: Option<String>,
    pub comment: Option<String>,
    pub bpm: Option<f64>,
    pub initial_key: Option<String>,
    pub mood: Option<String>,
    pub grouping: Option<String>,
    pub work: Option<String>,
    pub movement: Option<String>,
    pub movement_number: Option<i32>,
    pub movement_total: Option<i32>,
    pub language: Option<String>,
    pub copyright: Option<String>,
    pub isrc: Option<String>,
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

/// What a scrobble needs, native columns only; `ScrobbleTrack::from_row` converts it.
#[derive(Clone, Debug, PartialEq, FromRow)]
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

/// The FK ids for a surface rendering from [`TrackSummary`], a struct so a swapped album and
/// artist can't compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, FromRow)]
pub struct TrackLinks {
    pub id: i64,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub genre_id: Option<i64>,
}

/// Every column, for the few callers needing the whole row.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct Track {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    pub file_hash: Option<String>,

    // Core metadata
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    /// Rendered for the FTS index; the structured rows are `track_credits`.
    pub credits: Option<String>,
    pub track_number: Option<i32>,
    pub track_total: Option<i32>,
    pub disc_number: Option<i32>,
    pub disc_total: Option<i32>,
    pub disc_subtitle: Option<String>,
    pub subtitle: Option<String>,
    /// The whole release date; `year` is the integer derived from it and is what every index and
    /// smart-playlist rule reads.
    pub release_date: Option<String>,
    pub year: Option<i32>,
    pub original_date: Option<String>,
    pub original_year: Option<i32>,
    pub comment: Option<String>,

    // Extended metadata
    pub bpm: Option<f64>,
    pub initial_key: Option<String>,
    pub mood: Option<String>,
    pub grouping: Option<String>,
    pub work: Option<String>,
    pub movement: Option<String>,
    pub movement_number: Option<i32>,
    pub movement_total: Option<i32>,
    pub language: Option<String>,
    pub copyright: Option<String>,
    pub isrc: Option<String>,
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub musicbrainz_release_track_id: Option<String>,

    // ReplayGain
    pub replaygain_track_gain: Option<f64>,
    pub replaygain_track_peak: Option<f64>,
    pub replaygain_album_gain: Option<f64>,
    pub replaygain_album_peak: Option<f64>,

    // Technical info
    pub duration_ms: i64,
    pub file_size: Option<i64>,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,

    // Artwork
    pub artwork_path: Option<String>,

    // Playback state
    pub play_count: i32,
    pub skip_count: i32,
    pub rating: i32,
    pub is_favorite: bool,
    pub last_played: Option<String>,
    pub last_position: i64,

    // Relations
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub genre_id: Option<i64>,
    pub folder_id: i64,

    // Timestamps
    pub date_added: String,
    pub date_modified: Option<String>,

    // Precomputed natural sort key
    pub sort_key: Option<String>,
}

/// Aggregate stats for the Favorites hero header.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FavoriteStats {
    pub count: i64,
    pub total_duration_ms: i64,
    /// Distinct primary artists (`tracks.artist_id`), the spread the Songs tab's band states.
    pub artists: i64,
    /// Distinct albums (`tracks.album_id`).
    pub albums: i64,
    pub artwork_paths: Vec<String>,
}

/// A Most Played card, on both curated pages: what the card draws and nothing else, the filter
/// matching `tracks.search_text` in SQL.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct MostPlayedFavorite {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub artwork_path: Option<String>,
    pub play_count: i32,
    /// What the card's heart draws. Always set on Favorites' ranking, which is filtered on it;
    /// Recently Played's spans the whole library.
    pub is_favorite: bool,
}

#[cfg(test)]
#[path = "tests/track_tests.rs"]
mod tests;
