//! A scan's last write, and what follows a scan that completed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::library::settings::folders::NestedFolder;
use crate::services;
use crate::state::AppState;
use crate::tasks::{self, TaskSpawner};
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;
use melodia_store::database::queries;
use melodia_store::database::queries::ingest::IngestResult;
use melodia_store::media::ingest::scanner::MediaWalk;

/// What a scan's chunks wrote, summed across them, and how.
pub(super) struct Ingested {
    /// The chunks commit with the stats triggers dropped, leaving the counts to the final recalc.
    bulk: bool,
    pub inserted: u32,
    moved: u32,
    updated: u32,
}

impl Ingested {
    pub(super) fn new(bulk: bool) -> Self {
        Self { bulk, inserted: 0, moved: 0, updated: 0 }
    }

    pub(super) fn is_bulk(&self) -> bool {
        self.bulk
    }

    pub(super) fn add(&mut self, chunk: &IngestResult) {
        self.inserted += chunk.inserted_count;
        self.moved += chunk.moved_count;
        self.updated += chunk.updated_count;
    }

    pub(super) fn any(&self) -> bool {
        self.inserted > 0 || self.updated > 0 || self.moved > 0
    }

    pub(super) fn rewrote_existing(&self) -> bool {
        self.moved > 0 || self.updated > 0
    }
}

/// Orphan pruning, the album-artwork roll-up and the stats recalc, in one transaction.
///
/// `walk` is the folder's complete walk, the set its rows are checked against for files that
/// vanished. A stopped scan has none to hand over and purges nothing: every row its walk didn't
/// reach would read as an orphan. It still owes the rest, since on the bulk path each chunk
/// committed with the stats triggers dropped and only the recalc here brings the counts back.
pub(super) async fn commit_final(
    state: &AppState,
    folder: &Folder,
    walk: Option<MediaWalk>,
    ingested: &Ingested,
) -> Result<(), AppError> {
    let mut tx = state.db.write().begin().await?;
    let orphans = match walk {
        Some(walk) => {
            let db_paths =
                queries::scan::get_all_track_paths_for_folder(&mut tx, folder.id).await?;
            orphans_of(db_paths, walk)
        }
        None => Vec::new(),
    };
    // On the bulk path a full recalc follows anyway, so a large orphan
    // purge shouldn't pay per-row delete triggers on top — drop them for
    // the delete. Small deltas keep the triggers on (per-row maintenance
    // is the whole point of the `!is_bulk` branch).
    let is_bulk = ingested.is_bulk();
    let bulk_orphan_purge = is_bulk && !orphans.is_empty();
    if bulk_orphan_purge {
        queries::stats::disable_stats_triggers(&mut tx).await?;
    }
    if !orphans.is_empty() {
        log::info!("Removing {} orphaned tracks from folder {}", orphans.len(), folder.path);
        queries::scan::delete_tracks_by_paths_batch(&mut tx, &orphans).await?;
    }

    // No-op rescans (every file unchanged, no orphans, no inserts) skip the
    // artwork roll-ups and the full stats recalc, O(rows) sweeps that produce
    // identical values when nothing changed.
    let any_changes = ingested.any() || !orphans.is_empty();

    if any_changes {
        queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
        queries::playlist::fill_missing_thumbnails(&mut tx).await?;
        // Purged orphan tracks can leave their album/artist/genre empty; sweep those.
        queries::scan::prune_orphans(&mut tx).await?;
    }
    // Small deltas (`!is_bulk`) never dropped the triggers, so per-row
    // maintenance already kept the stats correct — no recalc needed at all.
    if is_bulk {
        if any_changes {
            queries::stats::recalculate_all_stats(&mut tx).await?;
        }
        if bulk_orphan_purge {
            queries::stats::enable_stats_triggers(&mut tx).await?;
        }
    }
    tx.commit().await?;

    if ingested.moved > 0 {
        log::info!("Detected {} moved/renamed files", ingested.moved);
    }
    if ingested.updated > 0 {
        log::info!("Updated metadata for {} changed files", ingested.updated);
    }
    Ok(())
}

/// Rows whose file is no longer on disk. Compared against the full on-disk set, not what was
/// parsed: the incremental filter leaves out unchanged files that are still present, and
/// treating those as orphans would delete the whole library. A row under a path the walk
/// couldn't read is spared for the same reason, its file being as likely there as gone.
fn orphans_of(db_paths: Vec<String>, walk: MediaWalk) -> Vec<String> {
    let on_disk: HashSet<String> = walk.files.into_iter().map(into_string_lossy).collect();
    db_paths
        .into_iter()
        .filter(|path| !on_disk.contains(path))
        .filter(|path| !walk.unreadable.iter().any(|dir| Path::new(path).starts_with(dir)))
        .collect()
}

/// Takes the path's own buffer, copying only for the rare path that isn't valid UTF-8.
fn into_string_lossy(path: PathBuf) -> String {
    path.into_os_string().into_string().unwrap_or_else(|path| path.to_string_lossy().into_owned())
}

/// Hands `folder` the folders nested in it, tracks and all, now that a scan of it has completed.
pub(super) async fn absorb_nested(
    state: &AppState,
    folder: &Folder,
    nested: &[NestedFolder],
) -> Result<(), AppError> {
    if nested.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = nested.iter().map(|f| f.id).collect();
    queries::folder::absorb_folders(&state.db, folder.id, &ids).await?;
    log::info!("Folded {} nested folder(s) into {}", ids.len(), folder.path);
    Ok(())
}

/// Stamps the folder as scanned and starts the passes a completed scan feeds. A stopped scan
/// owes neither: the folder isn't scanned, and the next scan of it runs them.
pub(super) async fn after_completed(state: &AppState, folder_id: i64) -> Result<(), AppError> {
    let now = melodia_core::utils::now_rfc3339();
    queries::folder::update_folder_last_scanned(&state.db, folder_id, &now).await?;

    services::artist_images::spawn_fetch(
        state.paths.clone(),
        state.db.clone(),
        state.http_client().clone(),
    );
    let spawner = TaskSpawner::from_state(state);
    tasks::retroactive_hash::spawn(&spawner, state);
    // After the orphan pass committed, so the rows it deleted are already gone from the
    // reference set this reads.
    tasks::artwork_sweep::spawn(&spawner, state);
    Ok(())
}
