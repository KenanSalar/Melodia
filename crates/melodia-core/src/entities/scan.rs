//! The scan pipeline's boundary values: what a walk found, what a file's tags said, and what the
//! database already holds for it.
//!
//! None of the three carries `FromRow` — [`super::search`] states the rule they follow, that a
//! value assembled by hand rather than decoded from a row is a boundary type rather than a row
//! type. They live here because the store's two halves both name them: `media/` produces
//! [`ExtractedMetadata`] and [`ScannedFile`] and consumes [`ExistingTrackSummary`], while
//! `database/queries/scan` does the reverse.

use std::path::PathBuf;

use super::artist::ArtistCredit;
use super::credits::RoleCredits;
use super::genre::GenreList;

/// Size and mtime for a track already in the database, feeding the incremental-scan filter that
/// decides whether an on-disk file is unchanged and can be skipped entirely.
///
/// See `media::ingest::scanner::track_is_current`, which compares `date_modified` byte-for-byte.
#[derive(Debug, Clone)]
pub struct ExistingTrackSummary {
    pub file_size: Option<i64>,
    pub date_modified: Option<String>,
}

/// One file the walk found, paired with what its tags read back as.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub metadata: ExtractedMetadata,
}

/// What a file says about the release it belongs to, rather than about itself.
///
/// Its own struct because `upsert_album` is the single consumer of all of it: one argument that
/// cannot be passed in the wrong order, over seven adjacent `Option<String>`s that can. Every
/// field lands under `COALESCE(excluded.x, albums.x)`, so the first track of a release carrying
/// one wins and a later track missing it does not blank it.
#[derive(Debug, Clone, Default)]
pub struct ReleaseTags {
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub barcode: Option<String>,
    /// Physical medium — "CD", "Digital Media", "12\" Vinyl".
    pub media: Option<String>,
    /// `MusicBrainz` release group type; multi-valued in the wild ("Album; Soundtrack"), kept as
    /// the string the file spelled because nothing filters on it yet.
    pub release_type: Option<String>,
    pub release_country: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
    /// `TCMP` / `COMPILATION` / `cpil`. A `bool` rather than an `Option<bool>`: a file that says
    /// nothing is not a compilation, and the column has always been `NOT NULL DEFAULT FALSE`.
    pub is_compilation: bool,
}

/// The filing order a file asks for, where it asks for one.
///
/// Grouped rather than flat for the reason four adjacent same-typed `Option<String>`s are a
/// hazard: a bind landing one slot over is invisible, and "The Beatles" filed under T instead of B
/// is exactly the symptom these exist to prevent. Each falls back to the locally derived sort key
/// when absent, so a library whose files carry none behaves as it does today.
#[derive(Debug, Clone, Default)]
pub struct SortTags {
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
}

/// Everything one file's tags and properties yield, in the shape the ingest queries bind from.
#[derive(Debug, Clone)]
pub struct ExtractedMetadata {
    pub title: String,
    pub artist: ArtistCredit,
    pub album_artist: ArtistCredit,
    /// `MusicBrainz` artist ids the file listed in parallel with its artist names. Either empty,
    /// or exactly as long as `artist.artists()` — the reader drops the whole list when the file's
    /// own counts disagree, because a misaligned id writes the wrong `MBID` onto a real artist.
    pub artist_mbids: Vec<String>,
    pub album_artist_mbids: Vec<String>,
    pub album: Option<String>,
    pub genres: GenreList,
    /// Composer, conductor, producer and the rest. One set rather than ten fields; the roles are
    /// [`super::credits::ROLES`].
    pub credits: RoleCredits,
    pub sort: SortTags,
    pub release: ReleaseTags,
    pub track_number: Option<i32>,
    pub track_total: Option<i32>,
    pub disc_number: Option<i32>,
    pub disc_total: Option<i32>,
    pub disc_subtitle: Option<String>,
    pub subtitle: Option<String>,
    /// Whole release date, and the year derived from it. Both, because `year` is what every
    /// existing index, `ORDER BY` and smart-playlist rule is built on, and the month and day are
    /// what the reader used to throw away.
    pub release_date: Option<String>,
    pub year: Option<i32>,
    pub original_date: Option<String>,
    pub original_year: Option<i32>,
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
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub musicbrainz_release_track_id: Option<String>,
    pub replaygain_track_gain: Option<f64>,
    pub replaygain_track_peak: Option<f64>,
    pub replaygain_album_gain: Option<f64>,
    pub replaygain_album_peak: Option<f64>,
    /// Stars the file's own tag carries, `None` when it carries none. Seeds a new row and,
    /// through `update_track_metadata`, overwrites an existing one — but only when it is
    /// `Some`, a rating with no carrier having nowhere else to live.
    pub rating: Option<i32>,
    pub duration_ms: i64,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub file_size: i64,
    pub file_hash: String,
    pub date_modified: Option<String>,
    pub artwork_path: Option<String>,
}
