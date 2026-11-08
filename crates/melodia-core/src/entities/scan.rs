//! The scan pipeline's boundary values, assembled rather than decoded, so none carries `FromRow`.
//! Here because the store's two halves both name them: `media/` produces [`ExtractedMetadata`]
//! and [`ScannedFile`] and reads [`FileStamp`], and `database/queries/scan` the reverse.

use std::path::PathBuf;

use super::artist::ArtistCredit;
use super::credits::RoleCredits;
use super::genre::GenreList;

/// The size and mtime the store last read a file at, a track's or a kept-out file's: a file that
/// still matches them is unchanged and isn't read again.
#[derive(Debug, Clone)]
pub struct FileStamp {
    pub file_size: Option<i64>,
    pub date_modified: Option<String>,
}

impl FileStamp {
    pub fn borrowed(&self) -> FileStampRef<'_> {
        FileStampRef { file_size: self.file_size, date_modified: self.date_modified.as_deref() }
    }

    /// [`FileStampRef::matches`] on this stamp.
    pub fn matches(&self, file_size: i64, date_modified: Option<&str>) -> bool {
        self.borrowed().matches(file_size, date_modified)
    }
}

/// A [`FileStamp`] with its mtime borrowed, for a holder keeping its stamps' text in one buffer.
#[derive(Debug, Clone, Copy)]
pub struct FileStampRef<'a> {
    pub file_size: Option<i64>,
    pub date_modified: Option<&'a str>,
}

impl FileStampRef<'_> {
    /// Whether a file of `file_size` bytes, modified at `date_modified`, is the one stamped. The
    /// mtime compares byte for byte, and a file with none never matches.
    pub fn matches(&self, file_size: i64, date_modified: Option<&str>) -> bool {
        self.file_size == Some(file_size)
            && date_modified.is_some()
            && self.date_modified == date_modified
    }
}

/// One file the walk found, paired with what its tags read back as.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub metadata: ExtractedMetadata,
}

/// What a file says about its release, one argument for `upsert_album` rather than seven adjacent
/// `Option<String>`s. The first track carrying a field wins, and a later one missing it doesn't
/// blank it.
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

/// The filing order a file asks for, grouped so a bind landing one slot over can't file "The
/// Beatles" under T. Each falls back to the locally derived sort key.
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
    /// The whole date beside the derived `year`, which every index, `ORDER BY` and smart-playlist
    /// rule is built on.
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

#[cfg(test)]
#[path = "tests/scan_tests.rs"]
mod tests;
