//! The scan pipeline's boundary values: what a walk found, what a file's tags said, and what the
//! database already holds for it.
//!
//! None of the three carries `FromRow` — [`super::search`] states the rule they follow, that a
//! value assembled by hand rather than decoded from a row is a boundary type rather than a row
//! type. They live here because the store's two halves both name them: `media/` produces
//! [`ExtractedMetadata`] and [`ScannedFile`] and consumes [`ExistingTrackSummary`], while
//! `database/queries/scan` does the reverse.

use std::path::PathBuf;

/// Size and mtime for a track already in the database, feeding the incremental-scan filter that
/// decides whether an on-disk file is unchanged and can be skipped entirely.
///
/// See `media::scanner::track_is_current`, which compares `date_modified` byte-for-byte.
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

/// Everything one file's tags and properties yield, in the shape the ingest queries bind from.
#[derive(Debug, Clone)]
pub struct ExtractedMetadata {
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub bpm: Option<f64>,
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub label: Option<String>,
    pub original_year: Option<i32>,
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
