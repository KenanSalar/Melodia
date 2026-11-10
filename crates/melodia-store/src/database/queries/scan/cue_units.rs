//! A scan chunk's cue sheet writes, on its transaction: each sheet's stamp, the rows it carves out
//! of the images it claims, and the images it gives back.
//!
//! A sheet track is keyed on its image and its number, so a reread sheet updates its rows in place
//! and a corrected time or title keeps every row's play count, rating and playlist places. When a
//! sheet first splits an image, the playlists holding the whole image take its tracks in the same
//! place; when it lets the image go, they take the image back where its first track sat. A play
//! count, rating or favourite on the side going away stays with it: it belongs to no single track.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use rusqlite::Transaction;

use super::{
    CuePlacement, NameCache, NewTrackRow, file_name_of, insert_tracks_batch, resolve_track_context,
    update_cue_track,
};
use crate::database::chunked_in_execute;
use crate::database::queries::excluded_files;
use crate::database::queries::playlist_carry::{merge_into_playlists, replace_in_playlists};
use crate::database::queries::track::file_rows::WHOLE_FILE;
use crate::database::sql;
use crate::media::cue::{Claim, CueChunk, CueRow, SheetJob, SheetRead, rows};
use melodia_core::entities::cue::CueTrack;
use melodia_core::entities::scan::{FileStamp, ScannedFile};
use melodia_core::error::AppError;

/// What a chunk's sheets wrote.
#[derive(Default)]
pub(crate) struct CueWrites {
    pub(crate) inserted: u32,
    /// Rows rewritten, whole-file rows a sheet replaced, and images given back.
    pub(crate) updated: u32,
}

/// Applies `cue`'s jobs for a scan of `folder_id`, each claimed image read in `images`.
///
/// Runs after the chunk's ordinary ingest, which wrote the whole-file row a released image takes
/// its playlist places back onto.
pub(crate) fn apply_sheet_jobs(
    tx: &Transaction<'_>,
    folder_id: i64,
    cue: &CueChunk,
    images: &[ScannedFile],
    scan_timestamp: &str,
) -> Result<CueWrites, AppError> {
    let mut writer = SheetWriter {
        tx,
        folder_id,
        scan_timestamp,
        images: images.iter().map(|image| (image.path.as_path(), image)).collect(),
        names: NameCache::for_chunk(images.len()),
        writes: CueWrites::default(),
    };
    for job in cue.jobs() {
        if let Some(read) = &job.read {
            writer.apply_read_sheet(job, read)?;
        }
        for image in &job.releases {
            if release_image(tx, &image.to_string_lossy())? {
                writer.writes.updated += 1;
            }
        }
    }
    Ok(writer.writes)
}

struct SheetWriter<'a, 'conn> {
    tx: &'a Transaction<'conn>,
    folder_id: i64,
    scan_timestamp: &'a str,
    images: HashMap<&'a Path, &'a ScannedFile>,
    names: NameCache,
    writes: CueWrites,
}

impl SheetWriter<'_, '_> {
    fn apply_read_sheet(&mut self, job: &SheetJob, read: &SheetRead) -> Result<(), AppError> {
        let units: Vec<(&Claim, &ScannedFile, Vec<CueRow>)> = job
            .claims
            .iter()
            .filter_map(|claim| {
                let image = *self.images.get(claim.image.as_path())?;
                let carved = rows(&read.sheet, &claim.layout, &claim.image, &image.metadata)?;
                Some((claim, image, carved))
            })
            .collect();
        // An image that couldn't be read, or that is too short for the sheet to split, keeps its
        // rows and leaves the sheet unstamped, so the next scan reads both again.
        let stamp = (units.len() == job.claims.len()).then_some(&read.stamp);
        let sheet_id = upsert_sheet(self.tx, &job.path.to_string_lossy(), self.folder_id, stamp)?;
        for (claim, image, carved) in &units {
            self.apply_cue_unit(claim, image, sheet_id, carved)?;
        }
        Ok(())
    }

    /// Writes the rows a sheet carves out of `claim`'s image over whatever it held: its earlier
    /// sheet rows by number, dropping the numbers the sheet no longer has, and a whole-file row,
    /// whose playlist places they take. A kept-out image writes nothing but its exclusion's stamp.
    fn apply_cue_unit(
        &mut self,
        claim: &Claim,
        image: &ScannedFile,
        sheet_id: i64,
        carved: &[CueRow],
    ) -> Result<(), AppError> {
        let tx = self.tx;
        let path = claim.image.to_string_lossy();
        if excluded_files::id_at_on(tx, &path)?.is_some() {
            return excluded_files::restamp_on(tx, &[(claim.image.as_path(), &image.metadata)]);
        }

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
            let ids = resolve_track_context(tx, self.folder_id, &row.meta, &mut self.names)?;
            let cue = CuePlacement { track: row.track, span: row.span, sheet_id: Some(sheet_id) };
            if let Some(id) = earlier.remove(&row.track) {
                update_cue_track(tx, id, cue, &row.meta, &ids, &mut self.names)?;
                placed.push((row.track, id));
                continue;
            }
            new_tracks.push(row.track);
            new_rows.push(NewTrackRow {
                file_path: path.clone().into_owned(),
                file_name: file_name_of(&claim.image),
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

/// Gives the image at `image` back its own row: the playlists holding any of its sheet tracks take
/// the whole-file row the chunk's ingest wrote, and the tracks go. Whether it had any.
fn release_image(tx: &Transaction<'_>, image: &str) -> Result<bool, AppError> {
    let track_ids: Vec<i64> = sql::fetch_all(
        tx,
        "SELECT id FROM tracks WHERE file_path = ? AND cue_track IS NOT NULL",
        (image,),
    )?;
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
    log::info!("Gave {image} its own row back from its cue sheet");
    Ok(true)
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
