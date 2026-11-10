pub mod album;
pub mod artist;
pub mod artwork;
pub mod duplicate_dismissals;
pub mod entity_keyset;
pub mod entity_labels;
pub mod excluded_files;
#[doc(hidden)]
pub mod fixtures;
pub mod folder;
pub mod genre;
pub mod ingest;
pub mod outside;
pub mod playlist;
pub(crate) mod playlist_carry;
pub mod playlist_convert;
pub mod playlist_delete;
pub mod playlist_restore;
pub mod radio;
pub mod radio_removal;
pub mod scan;
pub mod search;
pub mod search_name;
pub mod smart_playlist;
pub mod stats;
pub mod track;

// Only types are part of the public contract. Functions are reached via their
// containing submodule (e.g. `queries::track::get_track_by_id`) so the call
// site documents which area of the schema it touches and renames stay local.
pub use ingest::IngestResult;
pub use scan::ResolvedIds;

/// Whether two spellings of a path differing only in case name one file, by each platform's
/// default: a case-sensitive APFS volume, or a vfat mount on Linux, answers wrongly.
pub(crate) const DISK_IGNORES_CASE: bool = cfg!(any(target_os = "windows", target_os = "macos"));
