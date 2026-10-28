//! The flag struct behind Settings ▸ Library.

use serde::{Deserialize, Serialize};

/// Two switches ship on because off is the surprising state. Unwatched folders go stale in a way a
/// user can't diagnose, the toggle surviving for a huge library's inotify budget; and a star kept
/// only in the database is lost to a rebuild and invisible to other players.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "five independent settings.json keys, three of them one-shot markers; any grouping would be a container invented for the lint rather than one describing something"
)]
pub struct LibraryFlags {
    pub folder_watching_enabled: bool,
    /// Whether the artwork store has been brought inside its size bounds. The writer bounds new
    /// files only, `scanner::track_is_current` skipping unchanged ones, so the pass over the rest
    /// is one-shot and marked here, the store having no cheap way to say it was swept.
    pub artwork_store_normalized: bool,
    /// Whether a star set here is written into the file's tag too. A switch, since it makes a
    /// click rewrite the file.
    pub write_ratings_to_tags: bool,
    /// Whether the files' own ratings have been read in once, `track_is_current` skipping an
    /// unchanged file. Marked, since an unrated row can't be told from one the user cleared.
    pub ratings_imported_from_tags: bool,
    /// Whether the files' tags have been re-read since the ingest widened: the migrations seeded
    /// only what the database knew, and `track_is_current` never re-reads on its own.
    pub tags_backfilled: bool,
}

impl Default for LibraryFlags {
    fn default() -> Self {
        Self {
            folder_watching_enabled: true,
            artwork_store_normalized: false,
            write_ratings_to_tags: true,
            ratings_imported_from_tags: false,
            tags_backfilled: false,
        }
    }
}
