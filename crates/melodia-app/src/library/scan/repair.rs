//! What a scan does first: clear every reference to cover art whose stored file is gone.
//!
//! The inverse of `tasks::artwork_sweep`, which deletes files no row names. Every refill the
//! library has fills an empty column and leaves a set one alone, so a path to a deleted file
//! stopped all of them, and the scan's size and mtime gate never re-reads an unchanged track to
//! notice. Clearing the reference hands each column back to its own refill, the track's being the
//! scan that follows.
//!
//! A row naming a data root that has since moved is re-pointed rather than cleared: its file is in
//! the current store under the same content-addressed name, and clearing it would cost a re-parse,
//! an artist-image refetch and a custom playlist mosaic outright.

use std::path::{Path, PathBuf};

use rayon::iter::Either;
use rayon::prelude::*;
use tokio::sync::watch;

use crate::state::AppState;
use melodia_core::error::AppError;
use melodia_store::database::queries;
use melodia_store::media::ingest::scan_pool::ScanPool;

/// Raises `AppState::artwork_restoring` for as long as it lives, so a scan that stops or fails
/// part way can't leave the notice up.
pub(super) struct RestoreNotice(watch::Sender<u32>);

impl RestoreNotice {
    fn raise(restoring: &watch::Sender<u32>) -> Self {
        restoring.send_modify(|count| *count += 1);
        Self(restoring.clone())
    }
}

impl Drop for RestoreNotice {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count = count.saturating_sub(1));
    }
}

/// Re-points every library reference the current store still holds and clears the rest,
/// answering with the notice to hold while the scan puts the cleared covers back, or `None` when
/// nothing was cleared.
///
/// The album and playlist roll-ups run in the clearing transaction, so a cover that still exists on
/// another of an album's tracks is back before the scan starts rather than after it.
pub(super) async fn forget_missing(state: &AppState) -> Result<Option<RestoreNotice>, AppError> {
    let MissingReferences { relocated, gone } = missing_references(state).await?;
    if !relocated.is_empty() {
        let repointed = queries::artwork::repoint_all(&state.db, &relocated).await?;
        log::info!("Re-pointed {repointed} artwork reference(s) at the current data directory");
    }
    if gone.is_empty() {
        return Ok(None);
    }

    let mut tx = state.db.write().begin().await?;
    let cleared = queries::artwork::forget_paths(&mut tx, &gone).await?;
    queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
    queries::playlist::refresh_automatic_thumbnails(&mut tx).await?;
    tx.commit().await?;

    log::info!(
        "Artwork store is missing {} file(s); cleared {cleared} reference(s) to restore",
        gone.len()
    );
    Ok(Some(RestoreNotice::raise(&state.artwork_restoring)))
}

/// Starts a library reconcile when a stored cover the library names is gone, for a cache that
/// found one missing while the app runs. The reconcile's own repair does the work; this only
/// keeps a report about a file outside the library's columns, a station logo, from starting a
/// walk that would find nothing to do.
pub async fn restore_missing_artwork(state: &AppState) -> Result<(), AppError> {
    if !missing_references(state).await?.is_empty() {
        super::reconcile_watched_folders(state);
    }
    Ok(())
}

/// The library's artwork references whose file is not where the row says.
struct MissingReferences {
    /// `(stored, current)` for a file the current store holds under the same name.
    relocated: Vec<(String, String)>,
    /// Gone from every store, so only a scan can put them back.
    gone: Vec<String>,
}

impl MissingReferences {
    fn is_empty(&self) -> bool {
        self.relocated.is_empty() && self.gone.is_empty()
    }
}

/// One `stat` per distinct path, and one per store for each that fails it.
///
/// Only a definite not-found counts: a `stat` failing for any other reason is no proof the file is
/// gone, and clearing a reference costs a re-parse, or a custom playlist image outright.
async fn missing_references(state: &AppState) -> Result<MissingReferences, AppError> {
    let referenced = queries::artwork::referenced_library_paths(&state.db).await?;
    let stores = [state.paths.artwork_dir.clone(), state.paths.artists_dir.clone()];
    let (relocated, gone) = tokio::task::spawn_blocking(move || {
        ScanPool::for_files(referenced.len()).install(|| {
            referenced
                .into_par_iter()
                .filter(|path| matches!(Path::new(path).try_exists(), Ok(false)))
                .partition_map(|path| match current_copy(&path, &stores) {
                    Some(current) => Either::Left((path, current)),
                    None => Either::Right(path),
                })
        })
    })
    .await
    .map_err(|e| AppError::scanner("Artwork check task failed", e))?;
    Ok(MissingReferences { relocated, gone })
}

/// Where the current store holds the file `path` names, the name alone identifying it.
fn current_copy(path: &str, stores: &[PathBuf]) -> Option<String> {
    let name = Path::new(path).file_name()?;
    stores
        .iter()
        .map(|dir| dir.join(name))
        .find(|candidate| matches!(candidate.try_exists(), Ok(true)))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}
