use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct AlbumStats {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub artist_id: i64,
    pub artist_name: String,
    pub year: Option<i32>,
    pub disc_count: Option<i32>,
    pub is_compilation: bool,
    pub musicbrainz_id: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub barcode: Option<String>,
    pub media: Option<String>,
    pub release_type: Option<String>,
    pub release_country: Option<String>,
    pub artwork_path: Option<String>,
    pub track_count: i32,
    pub total_duration_ms: i64,
}

/// The release-level tags behind one track, for the Edit-Tags dialog.
///
/// They are stored on `albums` but written per *file*, so the dialog reads them through the album
/// the track sits on and writes them back to every selected file. Strings rather than `Option`s
/// because the form folds a selection with `common_str`, which has no third state to show for a
/// NULL that a blank box doesn't already mean.
#[derive(Clone, Debug, Default, PartialEq, FromRow, Serialize, Deserialize)]
pub struct ReleaseTagRow {
    pub label: String,
    pub catalog_number: String,
    pub barcode: String,
    pub media: String,
    pub release_type: String,
    pub release_country: String,
    pub is_compilation: bool,
}
