pub mod album;
pub mod artist;
pub mod artwork;
pub mod folder;
pub mod genre;
pub mod ingest;
pub mod playlist;
pub mod scan;
pub mod search;
pub mod smart_playlist;
pub mod stats;
pub mod track;

// Only types are part of the public contract. Functions are reached via their
// containing submodule (e.g. `queries::track::get_track_by_id`) so the call
// site documents which area of the schema it touches and renames stay local.
pub use ingest::{FolderResolution, IngestResult};
pub use scan::ResolvedIds;

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod helpers;
}
