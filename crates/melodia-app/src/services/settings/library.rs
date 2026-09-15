//! The flag struct behind the Settings ▸ Library tab.

use serde::{Deserialize, Serialize};

/// Library-management toggles.
///
/// Two default-on switches, both because the off state is the surprising one.
/// `folder_watching_enabled`: every consumer player auto-watches with no toggle at
/// all, and watching off lands in a stale-UI failure mode a user can't diagnose —
/// the toggle survives as an escape valve for the inotify watch budget on huge
/// libraries. `write_ratings_to_tags`: a star that lives only in this database is
/// one a library rebuild loses and no other player can see.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "six independent settings.json keys, four of them one-shot markers; any grouping would be a container invented for the lint rather than one describing something"
)]
pub struct LibraryFlags {
    pub folder_watching_enabled: bool,
    pub music_folder_auto_added: bool,
    /// Whether the artwork store has been brought inside its size bounds once.
    ///
    /// Normalization happens at the writer, so it only ever reaches newly-scanned files —
    /// `scanner::track_is_current` guarantees an unchanged track's artwork is never re-derived.
    /// A pass over what is already there is therefore one-shot rather than continuous, and
    /// marked here rather than inferred, there being no cheap way to ask the store whether it
    /// has been swept short of reading every file in it.
    pub artwork_store_normalized: bool,
    /// Whether a star set here is also written into the file's own tag.
    ///
    /// On, the rating is portable — other players read it, and it survives a library rebuild or
    /// a move to another machine, which is the whole reason the column alone was not enough.
    /// The cost is that a one-click action rewrites the file, so it is a switch rather than an
    /// assumption.
    pub write_ratings_to_tags: bool,
    /// Whether the ratings already sitting in this library's files have been read in once.
    ///
    /// `scanner::track_is_current` skips an unchanged file outright, so a library scanned before
    /// ratings were read stays unrated no matter how many times it is rescanned. The sweep that
    /// fixes that is one-shot, and marked here rather than inferred — an unrated row is
    /// indistinguishable from one the user deliberately cleared.
    pub ratings_imported_from_tags: bool,
    /// Whether the tags this library's files carry have been re-read once since the ingest
    /// widened.
    ///
    /// The migrations seed what the database already knew — one artist credit per track, one
    /// genre, a composer — and that is every name a library indexed before them holds. Everything
    /// the reader gained since is in the files, and `scanner::track_is_current` will never re-read
    /// them on its own.
    pub tags_backfilled: bool,
}

impl Default for LibraryFlags {
    fn default() -> Self {
        Self {
            folder_watching_enabled: true,
            music_folder_auto_added: false,
            artwork_store_normalized: false,
            write_ratings_to_tags: true,
            ratings_imported_from_tags: false,
            tags_backfilled: false,
        }
    }
}
