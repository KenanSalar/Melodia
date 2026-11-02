//! What the duplicate finder hands the UI: groups of tracks that look like one song, each labelled
//! with how sure the match is. The rule that builds them is `library::duplicates::matcher`'s.

use sqlx::FromRow;

use crate::entities::audio_quality::AudioFormat;

/// How a group's copies matched, which decides what is picked for removal unasked. Declared in
/// list order, the surest matches first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// Every copy is one byte-identical file.
    Identical,
    /// One recording on one release, as far as the tags tell.
    SameRelease,
    /// Copies that may be different songs; nothing is picked unasked.
    Review(ReviewReason),
}

/// Why a group needs a person to look. Declared in precedence: a group is labelled by the first
/// that holds, the strongest doubt that its copies are one song.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewReason {
    /// The title may be a file stem, an untagged file falling back to one.
    NoArtist,
    /// The recording MBIDs differ: a live take or a remix its length didn't tell apart.
    OtherRecording,
    /// The albums, years or release MBIDs differ: a compilation, or a remaster tagged like its
    /// original but for the year.
    OtherRelease,
    /// One album holding the title twice, as a deluxe edition can.
    OtherPosition,
}

/// Copies the user marked Not Duplicates together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(transparent)]
pub struct DismissalSet(i64);

/// One copy in a group: what the matcher compares, and what the dialog shows.
#[derive(Clone, Debug, FromRow)]
pub struct DuplicateCopy {
    pub id: i64,
    pub file_path: String,
    pub file_hash: Option<String>,
    pub file_size: Option<i64>,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub disc_number: Option<i32>,
    pub track_number: Option<i32>,
    pub year: Option<i32>,
    /// The recording MBID, whatever the column's name says.
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub duration_ms: i64,
    pub date_added: String,
    #[sqlx(flatten)]
    pub format: AudioFormat,
    pub dismissal: Option<DismissalSet>,
}

/// Tracks that look like one song.
#[derive(Clone, Debug)]
pub struct DuplicateGroup {
    pub kind: MatchKind,
    /// Best first, so the first is the copy the suggested picks keep.
    pub copies: Vec<DuplicateCopy>,
    /// Set while every copy carries one set, so a copy joining since shows the group again.
    pub dismissed: Option<DismissalSet>,
}
