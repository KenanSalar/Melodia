//! Where a new path's bytes may have come from: a row, a kept-out file or a cue sheet's image,
//! whose own path is gone. An image is one candidate however many tracks its sheet carves, so its
//! rows move together.

use std::cell::LazyCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;

use rusqlite::Connection;

use crate::database::queries::{excluded_files, folder, track};
use melodia_core::error::AppError;

type ByHash = HashMap<String, Vec<(i64, String)>>;

/// Candidates by hash, each taken once so two same-hash files can't both claim one.
#[derive(Default, Clone)]
pub(crate) struct MoveCandidates {
    /// Every row per hash, lowest id first, as `(id, old path)`: any of a hash's twins can be the
    /// one that moved.
    pub(crate) tracks: ByHash,
    exclusions: ByHash,
    /// Every file per hash a sheet may split, split already or still read whole, as
    /// `(its lowest row id, old path)`.
    images: ByHash,
    /// Every library folder, which tells a file on an offline drive from one that moved.
    library_roots: Vec<String>,
}

impl MoveCandidates {
    /// Every whole-file row and kept-out file carrying one of `hashes`, and every image carrying
    /// one of `image_hashes`, whether or not its file is still there.
    pub(crate) fn read_on(
        conn: &Connection,
        hashes: &[&str],
        image_hashes: &[&str],
    ) -> Result<Self, AppError> {
        Ok(Self {
            tracks: track::rows_by_hash_on(conn, hashes)?,
            exclusions: excluded_files::by_hash_on(conn, hashes)?,
            images: track::images_by_hash_on(conn, image_hashes)?,
            library_roots: folder::paths_on(conn)?,
        })
    }

    pub(crate) fn old_paths(&self) -> Vec<String> {
        let candidates = self.all().flat_map(HashMap::values).flatten();
        candidates.map(|(_, path)| path.clone()).collect()
    }

    fn all(&self) -> impl Iterator<Item = &ByHash> {
        [&self.tracks, &self.exclusions, &self.images].into_iter()
    }

    /// Keeps the candidates whose file is gone: one still on disk is a copy the new file sits
    /// beside, not where it came from.
    pub(crate) fn retain_gone(&mut self, still_present: &HashSet<String>) {
        self.drop_offline_twins(still_present);
        for candidates in [&mut self.tracks, &mut self.exclusions, &mut self.images] {
            candidates.retain(|_, files| {
                files.retain(|(_, path)| !still_present.contains(path));
                !files.is_empty()
            });
        }
    }

    /// Drops the candidates on an offline library folder from a hash with a file still on disk:
    /// the new file is a copy of that one, and a drive gone offline moved nothing. A hash with
    /// nothing left on disk keeps them, so a library folder moved and added back takes its history
    /// along.
    fn drop_offline_twins(&mut self, still_present: &HashSet<String>) {
        let on_disk: HashSet<String> = self
            .all()
            .flatten()
            .filter(|(_, files)| files.iter().any(|(_, path)| still_present.contains(path)))
            .map(|(hash, _)| hash.clone())
            .collect();
        // Lazy because a stat on an offline network share can hang for seconds.
        let offline_roots = LazyCell::new(|| {
            let roots = self.library_roots.iter().map(Path::new);
            roots.filter(|root| !holds_entries(root)).collect::<Vec<_>>()
        });
        let is_offline =
            |path: &str| offline_roots.iter().any(|root| Path::new(path).starts_with(root));
        for candidates in [&mut self.tracks, &mut self.exclusions, &mut self.images] {
            for (hash, files) in candidates.iter_mut() {
                if on_disk.contains(hash) {
                    files.retain(|(_, path)| still_present.contains(path) || !is_offline(path));
                }
            }
        }
    }

    /// Leaves out the kept-out files, for a caller that brings them back.
    pub(crate) fn forget_exclusions(&mut self) {
        self.exclusions.clear();
    }

    /// The row a new file at `new_path` with `hash`'s bytes moved from: the one sharing its file
    /// name, so twins moved together keep their own histories, else the lowest id. None where only
    /// a kept-out file shares the name, the new file being that one, moved beside its keeper.
    pub(crate) fn track_for(&self, hash: &str, new_path: &Path) -> Option<(i64, String)> {
        let rows = self.tracks.get(hash)?;
        let new_name = new_path.file_name();
        if let Some(same_name) = named(rows, new_name) {
            return Some(same_name.clone());
        }
        let is_kept_out =
            self.exclusions.get(hash).is_some_and(|files| named(files, new_name).is_some());
        if is_kept_out { None } else { rows.first().cloned() }
    }

    /// Spends the row `id`, so no later file with `hash`'s bytes re-points it too.
    pub(crate) fn spend_track(&mut self, hash: &str, id: i64) {
        let Some(rows) = self.tracks.get_mut(hash) else {
            return;
        };
        rows.retain(|(row_id, _)| *row_id != id);
        if rows.is_empty() {
            self.tracks.remove(hash);
        }
    }

    /// The image a new image at `new_path` with `hash`'s bytes moved from: the one sharing its file
    /// name, else the first in [`track::images_by_hash_on`]'s order.
    pub(crate) fn image_for(&self, hash: &str, new_path: &Path) -> Option<String> {
        let images = self.images.get(hash)?;
        let image = named(images, new_path.file_name()).or_else(|| images.first())?;
        Some(image.1.clone())
    }

    /// Spends the image at `old_path`, so no later image with `hash`'s bytes takes its rows too.
    pub(crate) fn spend_image(&mut self, hash: &str, old_path: &str) {
        let Some(images) = self.images.get_mut(hash) else {
            return;
        };
        images.retain(|(_, path)| path != old_path);
        if images.is_empty() {
            self.images.remove(hash);
        }
    }

    /// A kept-out file a new file with `hash`'s bytes moved from, as `(exclusion id, old path)`.
    pub(crate) fn take_exclusion(&mut self, hash: &str) -> Option<(i64, String)> {
        let files = self.exclusions.get_mut(hash)?;
        let taken = files.pop();
        if files.is_empty() {
            self.exclusions.remove(hash);
        }
        taken
    }
}

fn named<'a>(candidates: &'a [(i64, String)], name: Option<&OsStr>) -> Option<&'a (i64, String)> {
    candidates.iter().find(|(_, old)| Path::new(old).file_name() == name)
}

/// Whether a library folder has anything to read: an unmounted drive's mount point is there but
/// empty.
fn holds_entries(root: &Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_some())
}
