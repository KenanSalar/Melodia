//! Which rows a statement means: a cue sheet splits one file into a row per track, and a file
//! opened from outside every library folder keeps a row no library page shows. A statement that
//! moves or deletes a file means every row of its path and takes neither.

/// The row a file has when no sheet splits it: what a read or write of the file's own tags means.
pub(crate) const WHOLE_FILE: &str = "cue_track IS NULL";

/// A row a cue sheet carves out of its file.
pub(crate) const SHEET_TRACK: &str = "cue_track IS NOT NULL";

/// A row carved by a sheet in a `.cue` of its own.
pub(crate) const CARVED_BY_CUE: &str = "(cue_track IS NOT NULL AND cue_sheet_id IS NOT NULL)";

/// A row the sheet inside its image carved. `cue_sheet_id IS NULL` alone holds for a whole-file
/// row too.
pub(crate) const CARVED_INSIDE: &str = "(cue_track IS NOT NULL AND cue_sheet_id IS NULL)";

/// Files in the order a folder plays them (`track_ids_under_directory`), each file's rows in its
/// sheet's order.
pub(crate) const FILE_ORDER: &str = "file_path COLLATE NOCASE ASC, cue_track ASC";

macro_rules! outside_ids {
    () => {
        "SELECT outside.id FROM folders f \
         CROSS JOIN tracks outside ON outside.folder_id = f.id WHERE NOT f.is_enabled"
    };
}

/// The ids of the rows filed under a disabled folder, files opened from outside every library
/// folder. `CROSS JOIN` keeps the folders the outer loop, so the read takes only their rows off
/// `idx_tracks_folder_id` even before `ANALYZE` has run.
pub(crate) const OUTSIDE_IDS: &str = outside_ids!();

/// A row a library surface shows. Asked as the outside rows' ids rather than through the row's own
/// folder because every index carries the id: a list walking its order's index stays on it, where a
/// `folder_id` test sorts the whole library.
pub(crate) const IN_LIBRARY: &str = concat!("id NOT IN (", outside_ids!(), ")");

/// `date_modified` as a library scan compares it: a row with no links reads as never stamped, so
/// the scan writes it again and links it.
pub(crate) const LINKED_STAMP: &str = "IIF(artist_id IS NULL, NULL, date_modified)";
