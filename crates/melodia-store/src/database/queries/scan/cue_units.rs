//! The cue sheet writes of a scan chunk or a watcher batch, on its transaction: each sheet's stamp,
//! the rows it carves out of the images it claims, and the images it gives back. A sheet inside its
//! image writes the same rows with no stamp of its own, the image's standing for it.
//!
//! A sheet track is keyed on its image and its number, so a reread sheet updates its rows in place
//! and a corrected time or title keeps every row's play count, rating and playlist places. When a
//! sheet first splits an image, the playlists holding the whole image take its tracks in the same
//! place; when it lets the image go, they take the image back where its first track sat. A play
//! count, rating or favourite on the side going away stays with it: it belongs to no single track.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use rusqlite::Transaction;

use super::{
    CuePlacement, MoveCandidates, NameCache, NewTrackRow, TrackFolder, file_name_of,
    insert_tracks_batch, resolve_track_context, track_exists_by_path, update_cue_track,
    update_track_location,
};
use crate::database::queries::excluded_files;
use crate::database::queries::playlist_carry::{merge_into_playlists, replace_in_playlists};
use crate::database::queries::track::file_rows::WHOLE_FILE;
use crate::database::{chunked_in_execute, chunked_in_query, sql};
use crate::media::cue::{CueRow, SheetJob, SheetRead, rows};
use melodia_core::entities::cue::CueTrack;
use melodia_core::entities::scan::{FileStamp, ScannedFile};
use melodia_core::error::AppError;

/// What a batch's sheets wrote.
#[derive(Default)]
pub(crate) struct CueWrites {
    pub(crate) inserted: u32,
    /// Rows rewritten, whole-file rows a sheet replaced, and images given back.
    pub(crate) updated: u32,
    /// Images that took the rows of the path they moved from.
    pub(crate) moved: u32,
}

impl CueWrites {
    pub(crate) fn any(&self) -> bool {
        self.inserted > 0 || self.updated > 0 || self.moved > 0
    }
}

/// Writes sheet jobs on one transaction, each claimed image read in the batch's `images`.
pub(crate) struct SheetWriter<'a, 'conn> {
    tx: &'a Transaction<'conn>,
    scan_timestamp: &'a str,
    images: HashMap<&'a Path, &'a ScannedFile>,
    /// The gone images a claimed one may have moved from.
    moved: &'a mut MoveCandidates,
    names: NameCache,
    writes: CueWrites,
}

/// One image and the rows a sheet carves out of it.
struct CueUnit<'u> {
    image: &'u ScannedFile,
    rows: &'u [CueRow],
}

/// Where a sheet's rows are filed.
#[derive(Clone, Copy)]
struct Filing {
    /// `None` for the sheet inside the image.
    sheet_id: Option<i64>,
    folder: TrackFolder,
}

impl<'a, 'conn> SheetWriter<'a, 'conn> {
    pub(crate) fn new(
        tx: &'a Transaction<'conn>,
        images: &'a [ScannedFile],
        scan_timestamp: &'a str,
        moved: &'a mut MoveCandidates,
    ) -> Self {
        Self {
            tx,
            scan_timestamp,
            images: images.iter().map(|image| (image.path.as_path(), image)).collect(),
            moved,
            names: NameCache::for_chunk(images.len()),
            writes: CueWrites::default(),
        }
    }

    /// Applies `job`, filing its sheet and rows under `folder`.
    ///
    /// Runs after the batch's ordinary ingest, which wrote the whole-file row a released image
    /// takes its playlist places back onto.
    pub(crate) fn apply(&mut self, job: &SheetJob, folder: TrackFolder) -> Result<(), AppError> {
        if let Some(read) = &job.read {
            self.apply_read_sheet(job, read, folder)?;
        }
        for image in &job.releases {
            if release_rows(self.tx, &image.to_string_lossy(), Carver::Cue)? {
                self.writes.updated += 1;
            }
        }
        Ok(())
    }

    /// Writes the rows the sheet inside `image` carves out of it, filed under `folder`.
    pub(crate) fn apply_embedded(
        &mut self,
        image: &ScannedFile,
        rows: &[CueRow],
        folder: TrackFolder,
    ) -> Result<(), AppError> {
        self.apply_cue_unit(&CueUnit { image, rows }, Filing { sheet_id: None, folder })
    }

    pub(crate) fn into_writes(self) -> CueWrites {
        self.writes
    }

    fn apply_read_sheet(
        &mut self,
        job: &SheetJob,
        read: &SheetRead,
        folder: TrackFolder,
    ) -> Result<(), AppError> {
        let carved: Vec<(&ScannedFile, Vec<CueRow>)> = job
            .claims
            .iter()
            .filter_map(|claim| {
                let image = *self.images.get(claim.image.as_path())?;
                let rows = rows(&read.sheet, &claim.layout, &claim.image, &image.metadata)?;
                Some((image, rows))
            })
            .collect();
        // An image that couldn't be read, or that is too short for the sheet to split, keeps its
        // rows and leaves the sheet unstamped, so the next scan reads both again.
        let stamp = (carved.len() == job.claims.len()).then_some(&read.stamp);
        let sheet_id = upsert_sheet(self.tx, &job.path.to_string_lossy(), folder.id(), stamp)?;
        let filing = Filing { sheet_id: Some(sheet_id), folder };
        for (image, rows) in &carved {
            self.apply_cue_unit(&CueUnit { image, rows }, filing)?;
        }
        Ok(())
    }

    /// Writes the rows a sheet carves out of `unit`'s image over whatever it held: its earlier
    /// sheet rows by number, dropping the numbers the sheet no longer has, and a whole-file row,
    /// whose playlist places they take. A kept-out image writes nothing but its exclusion's stamp.
    fn apply_cue_unit(&mut self, unit: &CueUnit<'_>, filing: Filing) -> Result<(), AppError> {
        let tx = self.tx;
        let CueUnit { image, rows: carved } = *unit;
        let path = image.path.to_string_lossy();
        if excluded_files::id_at_on(tx, &path)?.is_some() {
            return excluded_files::restamp_on(tx, &[(image.path.as_path(), &image.metadata)]);
        }
        self.carry_moved_rows(image, filing.folder.id())?;

        let mut earlier: HashMap<CueTrack, i64> = sql::fetch_all(
            tx,
            "SELECT cue_track, id FROM tracks WHERE file_path = ? AND cue_track IS NOT NULL",
            (path.as_ref(),),
        )?
        .into_iter()
        .collect();
        let mut placed: Vec<(CueTrack, i64)> = Vec::with_capacity(carved.len());
        let mut new_rows = Vec::new();
        let mut new_tracks = Vec::new();
        for row in carved {
            let ids = resolve_track_context(tx, filing.folder, &row.meta, &mut self.names)?;
            let cue = CuePlacement { track: row.track, span: row.span, sheet_id: filing.sheet_id };
            if let Some(id) = earlier.remove(&row.track) {
                update_cue_track(tx, id, cue, &row.meta, &ids, &mut self.names)?;
                placed.push((row.track, id));
                continue;
            }
            new_tracks.push(row.track);
            new_rows.push(NewTrackRow {
                file_path: path.clone().into_owned(),
                file_name: file_name_of(&image.path),
                cue: Some(cue),
                meta: &row.meta,
                ids,
            });
        }
        self.writes.updated += u32::try_from(placed.len()).unwrap_or(u32::MAX);
        let new_ids = insert_tracks_batch(tx, &new_rows, self.scan_timestamp, &mut self.names)?;
        self.writes.inserted += u32::try_from(new_ids.len()).unwrap_or(u32::MAX);
        placed.extend(new_tracks.into_iter().zip(new_ids));

        let dropped: Vec<i64> = earlier.into_values().collect();
        chunked_in_execute(tx, &dropped, |id_list| {
            format!("DELETE FROM tracks WHERE id IN ({id_list})")
        })?;

        if let Some(whole_file) = whole_file_row(tx, &path)? {
            placed.sort_unstable_by_key(|&(track, _)| track);
            let in_sheet_order: Vec<i64> = placed.into_iter().map(|(_, id)| id).collect();
            replace_in_playlists(tx, whole_file, &in_sheet_order)?;
            sql::execute(tx, "DELETE FROM tracks WHERE id = ?", (whole_file,))?;
            self.writes.updated += 1;
        }
        Ok(())
    }

    /// Re-points the rows of the gone image `image`'s bytes came from, where its own path holds
    /// none yet, so a moved album keeps its tracks' play counts, ratings and playlist places.
    fn carry_moved_rows(&mut self, image: &ScannedFile, folder_id: i64) -> Result<(), AppError> {
        let hash = image.metadata.file_hash.as_str();
        let Some(old_path) = self.moved.image_for(hash, &image.path) else {
            return Ok(());
        };
        let path = image.path.to_string_lossy();
        if track_exists_by_path(self.tx, &path)? {
            return Ok(());
        }
        self.moved.spend_image(hash, &old_path);
        let repointed = update_track_location(
            self.tx,
            &old_path,
            &path,
            &file_name_of(&image.path),
            folder_id,
            image.metadata.date_modified.as_deref(),
        )?;
        if repointed {
            self.writes.moved += 1;
            log::info!("Detected moved file: {old_path} -> {path}");
        }
        Ok(())
    }
}

/// Records a sheet's stamp, answering its id. A `None` stamp matches no file, so the sheet is read
/// again.
fn upsert_sheet(
    tx: &Transaction<'_>,
    path: &str,
    folder_id: i64,
    stamp: Option<&FileStamp>,
) -> Result<i64, AppError> {
    sql::fetch_one(
        tx,
        "INSERT INTO cue_sheets (path, folder_id, file_size, date_modified) VALUES (?, ?, ?, ?)
         ON CONFLICT (path) DO UPDATE SET folder_id = excluded.folder_id,
             file_size = excluded.file_size, date_modified = excluded.date_modified
         RETURNING id",
        (
            path,
            folder_id,
            stamp.and_then(|stamp| stamp.file_size),
            stamp.and_then(|stamp| stamp.date_modified.as_deref()),
        ),
    )
}

/// Which sheet carved a row: one sheet's release leaves the other's rows alone, so an image whose
/// `.cue` goes keeps the rows the sheet inside it carved in that same chunk.
#[derive(Clone, Copy)]
enum Carver {
    Cue,
    Inside,
}

impl Carver {
    fn rows(self) -> &'static str {
        match self {
            Self::Cue => {
                "SELECT id FROM tracks \
                 WHERE file_path = ? AND cue_track IS NOT NULL AND cue_sheet_id IS NOT NULL"
            }
            Self::Inside => {
                "SELECT id FROM tracks \
                 WHERE file_path = ? AND cue_track IS NOT NULL AND cue_sheet_id IS NULL"
            }
        }
    }

    fn sheet(self) -> &'static str {
        match self {
            Self::Cue => "its cue sheet",
            Self::Inside => "the cue sheet it no longer carries",
        }
    }
}

/// Gives the image at `image` back its own row: the playlists holding any of the tracks `carver`'s
/// sheet carved take the whole-file row the batch wrote first, and those tracks go. Whether it had
/// any.
fn release_rows(tx: &Transaction<'_>, image: &str, carver: Carver) -> Result<bool, AppError> {
    let track_ids: Vec<i64> = sql::fetch_all(tx, carver.rows(), (image,))?;
    if track_ids.is_empty() {
        return Ok(false);
    }
    // None where the image is kept out or failed to read, and its tracks go all the same.
    if let Some(whole_file) = whole_file_row(tx, image)? {
        merge_into_playlists(tx, &track_ids, whole_file)?;
    }
    chunked_in_execute(tx, &track_ids, |id_list| {
        format!("DELETE FROM tracks WHERE id IN ({id_list})")
    })?;
    log::info!("Gave {image} its own row back from {}", carver.sheet());
    Ok(true)
}

/// Gives each of `images` its own row back from the sheet inside it, which its read no longer
/// finds, answering how many had tracks.
pub(crate) fn release_embedded(tx: &Transaction<'_>, images: &[&str]) -> Result<u32, AppError> {
    let mut released = 0;
    for image in images {
        released += u32::from(release_rows(tx, image, Carver::Inside)?);
    }
    Ok(released)
}

/// Which of `paths` hold tracks the sheet inside the file carved.
pub(crate) fn embedded_images_among(
    tx: &Transaction<'_>,
    paths: &[&str],
) -> Result<HashSet<String>, AppError> {
    let images: Vec<String> = chunked_in_query(tx, paths, |path_list| {
        format!(
            "SELECT DISTINCT file_path FROM tracks WHERE file_path IN ({path_list}) \
             AND cue_track IS NOT NULL AND cue_sheet_id IS NULL"
        )
    })?;
    Ok(images.into_iter().collect())
}

static WHOLE_FILE_ROW: LazyLock<String> =
    LazyLock::new(|| format!("SELECT id FROM tracks WHERE file_path = ? AND {WHOLE_FILE}"));

fn whole_file_row(tx: &Transaction<'_>, image: &str) -> Result<Option<i64>, AppError> {
    sql::fetch_optional(tx, WHOLE_FILE_ROW.as_str(), (image,))
}

/// Forgets the sheets at `paths`, whose files are gone, answering how many. A row still naming one
/// goes with it, its image gone too.
pub(crate) fn forget_sheets(tx: &Transaction<'_>, paths: &[String]) -> Result<u64, AppError> {
    chunked_in_execute(tx, paths, |path_list| {
        format!("DELETE FROM cue_sheets WHERE path IN ({path_list})")
    })
}

/// Forgets the gone sheet at `path` once no row names it, its images given back or claimed by
/// another sheet, answering whether it did. A row still naming it is an image no job settled, kept
/// with the stamp row for the next scan to give back rather than cascaded away.
pub(crate) fn forget_gone_sheet(tx: &Transaction<'_>, path: &str) -> Result<bool, AppError> {
    let forgotten = sql::execute(
        tx,
        "DELETE FROM cue_sheets WHERE path = ?
         AND NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.cue_sheet_id = cue_sheets.id)",
        (path,),
    )?;
    Ok(forgotten > 0)
}
