use serde::{Deserialize, Serialize};

use crate::entities::track::TrackListRow;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowseFolder {
    pub name: String,
    pub path: String,
}

/// One audio file in a browsed directory. The Browse view renders these
/// through the shared `TrackList` component, so every file — whether or not
/// it is in the library — carries a full [`TrackListRow`].
///
/// * In-library files (`in_library == true`) carry the real DB row.
/// * Disk-only files (`in_library == false`) carry a *synthesized* sparse
///   `TrackListRow` with `id == 0`, `title`/`file_name` taken from disk and
///   every other field empty/`None`/`0`. They render dimmed and
///   non-interactive (the UI keys interactivity off `id == 0`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowseFile {
    pub row: TrackListRow,
    pub in_library: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowseResult {
    pub path: String,
    pub name: String,
    pub folders: Vec<BrowseFolder>,
    pub files: Vec<BrowseFile>,
}
