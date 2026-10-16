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

/// One audio file in a browsed directory. The Browse view renders these
/// through the shared `TrackList` component, so every file — whether or not
/// it is in the library — carries a full [`TrackListRow`].
///
/// * In-library files (`in_library == true`) carry the real DB row.
/// * Disk-only files (`in_library == false`) carry a *synthesized* sparse
///   `TrackListRow` with `id == 0`, `title` taken from the file name and
///   every other field empty/`None`/`0`. They render dimmed and
///   non-interactive (the UI keys interactivity off `id == 0`).
///
/// `file_name` comes from disk for both, and breaks the view's sort ties.
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
