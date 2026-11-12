//! Scan-time database operations: the three write transactions a library change lands in, and the
//! pieces they are built from.
//!
//! [`commit_scan`], [`commit_retag`] and [`apply_watch_batch`] each own their transaction, and are
//! the only writes here another crate can reach: every query taking a transaction is `pub(crate)`.
//! A scan chunk's cue sheet writes run on `ingest::commit_scan_chunk`'s, a watcher batch's on
//! [`apply_watch_batch`]'s.

mod cue_units;
mod finish;
mod lookups;
mod moves;
mod mutations;
mod name_cache;
mod retag;
mod sort_key;
mod upserts;
mod watch_batch;

pub(crate) use cue_units::{CueWrites, SheetWriter, embedded_images_among, release_embedded};
pub use finish::{Unwalked, commit_scan, purge_unwalked};
pub use lookups::{
    SheetsAround, fold_folder_sheets, fold_folder_tracks, sheet_claims, sheets_around,
};
pub(crate) use lookups::{find_folder_for_path, track_exists_by_path};
pub(crate) use moves::MoveCandidates;
pub use mutations::{CuePlacement, INSERT_CHUNK_ROWS, NewTrackRow};
pub(crate) use mutations::{
    delete_track_by_path, delete_tracks_by_ids, delete_tracks_by_paths_batch, file_name_of,
    insert_track, insert_tracks_batch, prune_orphans, roll_up_covers,
    update_album_artwork_from_tracks, update_cue_track, update_track_location,
    update_track_metadata,
};
pub(crate) use name_cache::NameCache;
pub use retag::{Retag, RetaggedFile, commit_retag};
pub use sort_key::to_natural_sort_key;
pub(crate) use upserts::upsert_album;
pub use upserts::{CreditDetails, album_artist_name_for, album_credit_for};
pub use watch_batch::{WatchedChange, apply_watch_batch};

/// The folder a track is filed under, and whether the library holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrackFolder {
    /// A library folder, whose tracks link to their album, artists and genres.
    Library(i64),
    /// The disabled folder a file opened from outside every library folder is filed under. Its
    /// tracks link to nothing, so no album, artist or genre page counts them.
    Outside(i64),
}

impl TrackFolder {
    pub fn id(self) -> i64 {
        match self {
            Self::Library(id) | Self::Outside(id) => id,
        }
    }
}

/// A track's folder and, where the library holds it, the rows it links to.
#[derive(Clone, Copy)]
pub struct ResolvedIds {
    pub folder_id: i64,
    pub links: Option<Links>,
}

/// The album, artist and genre rows a library track links to. Its artist is never absent, Unknown
/// at worst, so a NULL `artist_id` marks a row with no links.
#[derive(Clone, Copy)]
pub struct Links {
    pub artist_id: i64,
    pub album_id: Option<i64>,
    pub genre_id: Option<i64>,
}

impl ResolvedIds {
    pub(crate) fn outside(folder_id: i64) -> Self {
        Self { folder_id, links: None }
    }

    pub fn artist_id(&self) -> Option<i64> {
        self.links.map(|links| links.artist_id)
    }

    pub fn album_id(&self) -> Option<i64> {
        self.links.and_then(|links| links.album_id)
    }

    pub fn genre_id(&self) -> Option<i64> {
        self.links.and_then(|links| links.genre_id)
    }
}

/// Resolves a track's artist, album and genre rows, upserting what's missing, beside the folder it
/// is filed under. A track filed outside the library resolves none, ahead of the lookups, which
/// write to the library's albums and artists.
pub(crate) fn resolve_track_context(
    tx: &rusqlite::Transaction<'_>,
    folder: TrackFolder,
    meta: &melodia_core::entities::scan::ExtractedMetadata,
    names: &mut NameCache,
) -> Result<ResolvedIds, melodia_core::error::AppError> {
    let folder_id = match folder {
        TrackFolder::Library(id) => id,
        TrackFolder::Outside(id) => return Ok(ResolvedIds::outside(id)),
    };
    // The **first** credited name: a row named "X feat. Y" is an artist nobody recorded under.
    let artist_name = meta.artist.primary_name();
    let album_name = meta.album.as_deref().unwrap_or("");
    // The first genre too, the rest reaching the library through `track_genres`.
    let genre_name = meta.genres.primary().unwrap_or("");

    let artist_id = names.artist(tx, artist_name, 1)?;
    // The album groups by its album artist, so a per-track featured credit doesn't split it.
    let album_artist_name = album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        names.artist(tx, album_artist_name, 1)?
    };
    let album_credit = album_credit_for(meta);
    let album_id = upsert_album(tx, album_name, album_artist_id, &album_credit, meta, names)?;
    let genre_id = names.genre(tx, genre_name)?;

    Ok(ResolvedIds { folder_id, links: Some(Links { artist_id, album_id, genre_id }) })
}

#[cfg(test)]
#[path = "../tests/scan_tests.rs"]
mod tests;
