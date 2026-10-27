use sqlx::FromRow;

use crate::entities::track::TrackListRow;

#[derive(Clone, Debug)]
pub struct BrowseFolder {
    pub name: String,
    pub path: String,
}

/// A library track directly inside a browsed directory, with the path Browse matches it to its
/// file on disk by.
#[derive(Debug, FromRow)]
pub struct DirectoryTrack {
    pub file_path: String,
    #[sqlx(flatten)]
    pub row: TrackListRow,
}

/// One audio file in a browsed directory, carrying a full [`TrackListRow`] so `TrackList` renders
/// it. A disk-only file's row is synthesized with `id == 0` and the file name as its title, which
/// the UI dims and disables. `file_name` comes from disk and breaks the sort's ties.
#[derive(Clone, Debug)]
pub struct BrowseFile {
    pub row: TrackListRow,
    pub file_name: String,
    pub in_library: bool,
}

#[derive(Clone, Debug)]
pub struct BrowseResult {
    pub path: String,
    pub name: String,
    pub folders: Vec<BrowseFolder>,
    pub files: Vec<BrowseFile>,
}
