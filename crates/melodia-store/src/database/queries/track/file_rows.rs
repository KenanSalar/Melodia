//! Which of a file's rows a statement means, now that a cue sheet splits one file into a row per
//! track. A statement that moves or deletes a file means every row of its path and takes neither.

/// The row a file has when no sheet splits it: what a read or write of the file's own tags means.
pub(crate) const WHOLE_FILE: &str = "cue_track IS NULL";

/// Files in the order a folder plays them (`track_ids_under_directory`), each file's rows in its
/// sheet's order.
pub(crate) const FILE_ORDER: &str = "file_path COLLATE NOCASE ASC, cue_track ASC";
