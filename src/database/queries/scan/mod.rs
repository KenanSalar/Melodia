//! Scan-time database operations: foreign-key upserts, track row
//! mutations, lookups, and the natural-sort-key helper.
//!
//! The four submodules are organised by *intent*: [`upserts`] handles
//! find-or-create on artist / album / genre rows, [`mutations`] holds
//! every track-row write, [`lookups`] holds read-side queries used by
//! the scanner / file-event processor, and [`sort_key`] precomputes the
//! `tracks.sort_key` column. Every public function is re-exported here
//! so callers continue to address them through `queries::scan::*`.

mod lookups;
mod mutations;
mod sort_key;
mod upserts;

pub use lookups::{
    ExistingTrackSummary, find_folder_for_path, get_all_track_paths_for_folder,
    get_existing_track_summaries_for_folder, get_track_id_by_path, track_exists_by_path,
};
pub use mutations::{
    INSERT_CHUNK_ROWS, NewTrackRow, delete_track_by_path, delete_tracks_by_paths_batch,
    insert_track, insert_tracks_batch, update_album_artwork_from_tracks,
    update_track_artwork_if_missing, update_track_location, update_track_metadata,
};
pub use sort_key::to_natural_sort_key;
pub use upserts::{upsert_album, upsert_artist, upsert_genre};

/// Resolved foreign-key IDs for a track being inserted during a scan.
pub struct ResolvedIds {
    pub artist_id: i64,
    pub album_id: Option<i64>,
    pub genre_id: Option<i64>,
    pub folder_id: i64,
}

#[cfg(test)]
#[path = "../tests/scan_tests.rs"]
mod tests;
