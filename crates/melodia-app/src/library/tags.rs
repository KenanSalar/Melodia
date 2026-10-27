//! Edit Track Information: rewrite the files, then land them through the scan pipeline
//! ([`extract_metadata`], then [`queries::scan::commit_retag`]), whose triggers reindex.
//!
//! Do **not** shortcut this into a hand-built `UPDATE` from the form values: a tag write rewrites
//! the file's bytes, and a fresh `date_modified` beside a stale `file_hash` is the one state
//! `scanner::track_is_current` reads as current forever. The re-extract keeps hash, size and mtime
//! honest together.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use rayon::prelude::*;

use crate::library::lyrics;
use crate::state::AppState;
use melodia_artwork::media::image::artwork::{self, CoverCache};
use melodia_core::entities::album::ReleaseTagRow;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::credits::RoleCredits;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::tags::{ArtworkEdit, TagEdit, TagField};
use melodia_core::entities::track::TagEditRow;
use melodia_core::error::AppError;
use melodia_core::error::describe;
use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::database::{DbPool, queries};
use melodia_store::media::ingest::metadata::extract_metadata;
use melodia_store::media::ingest::{cover_embed, tag_writer};

/// The MP4 save clones the embedded cover, so unbounded width would hold `num_cpus` × (image + its
/// clone) at once, and tag writing is I/O-bound anyway.
const TAG_WRITE_THREADS: usize = 4;

/// Built once: a rating write is usually one file, where spawning four threads per call is most of
/// the work, and the churn lands on the glibc arena the audio callback allocates from.
static TAG_WRITE_POOL: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(TAG_WRITE_THREADS)
        .thread_name(|i| format!("tag-write-{i}"))
        .build()
        .inspect_err(|e| {
            log::warn!("tag-write pool build failed ({}); writing sequentially", describe(e));
        })
        .ok()
});

/// Partial success is reported, never rolled back: a read-only file or an unsupported container
/// fails only its own row.
#[derive(Debug, Default)]
pub struct TagEditReport {
    pub updated: usize,
    /// `(file, error)` for files whose write or re-extract failed.
    pub failures: Vec<(String, String)>,
    /// A rare net, since all three primary tag types map every exposed field.
    pub unsupported: Vec<UnsupportedWrite>,
}

/// One file's worth of "that field did not fit in this container". The format rides along because
/// it is the reason, which the writer read off the file's header and a path can't tell the toast.
#[derive(Debug, Clone)]
pub struct UnsupportedWrite {
    pub path: String,
    /// `None` for a container lofty grew after this build, and the toast falls back to the count.
    pub format: Option<&'static str>,
    pub fields: Vec<TagField>,
}

impl TagEditReport {
    /// The one line a toast can draw when every file failed the same way; `None` where the batch
    /// spans containers or disagrees on fields, and the caller falls back to a count.
    #[must_use]
    pub fn unsupported_agreement(&self) -> Option<(&'static str, &[TagField])> {
        let (first, rest) = self.unsupported.split_first()?;
        let format = first.format?;
        let agreed =
            rest.iter().all(|other| other.format == first.format && other.fields == first.fields);
        agreed.then_some((format, &first.fields))
    }

    /// Log what the batch could not write, grouped as the toast groups it: a selection can be
    /// thousands of tracks against a log that rotates at a size, and one path per group is enough
    /// to chase. A failed commit returns before this, so a rolled-back edit logs none, the key
    /// that didn't fit not being what went wrong there.
    fn log_unsupported(&self) {
        let mut groups: Vec<(&UnsupportedWrite, usize)> = Vec::new();
        for write in &self.unsupported {
            match groups
                .iter_mut()
                .find(|(first, _)| first.format == write.format && first.fields == write.fields)
            {
                Some((_, others)) => *others += 1,
                None => groups.push((write, 0)),
            }
        }

        for (first, others) in groups {
            let what = tag_writer::describe_unsupported(first.format, &first.fields);
            match others {
                0 => log::warn!("{}: {what}", first.path),
                _ => log::warn!("{} and {others} more: {what}", first.path),
            }
        }
    }
}

/// The rows that populate the Edit Track Information dialog.
pub async fn get_tag_edit_rows(db: &DbPool, ids: &[i64]) -> Result<Vec<TagEditRow>, AppError> {
    queries::track::get_tag_edit_rows_by_ids(db, ids).await
}

/// A file's lyrics tag as the tag held it, judged by `library::lyrics`. Apart from
/// [`read_lyrics_and_credits`] because that caller wants nothing else. Blocking; the caller owns
/// the `spawn_blocking`.
pub fn read_lyrics(path: &Path) -> Result<Option<String>, AppError> {
    tag_writer::read_lyrics(path)
}

/// The lyrics tag and both artist credits off one open, for the dialog's single selection.
///
/// The credit comes from the file, not the database: a track whose join rows were seeded from
/// `artist_id` alone would otherwise open showing one name for a credit the file spells with three.
/// A multi-selection reads [`queries::track::get_track_credits_by_ids`] instead, N file reads on
/// the open path being what `TagEditRow` exists to avoid. Blocking; the caller owns the
/// `spawn_blocking`.
pub fn read_lyrics_and_credits(
    path: &Path,
) -> Result<(Option<String>, (ArtistCredit, ArtistCredit)), AppError> {
    tag_writer::read_lyrics_and_credits(path)
}

/// The credit behind each selected track, for the dialog's multi-selection path.
pub async fn get_tag_edit_credits(
    db: &DbPool,
    ids: &[i64],
) -> Result<HashMap<i64, (ArtistCredit, ArtistCredit)>, AppError> {
    queries::track::get_track_credits_by_ids(db, ids).await
}

/// From the database even for one track, unlike the artist credit: the lyrics tab's open doesn't
/// cover these, and the row is re-ingested from the file on every scan and save.
pub async fn get_tag_edit_role_credits(
    db: &DbPool,
    ids: &[i64],
) -> Result<HashMap<i64, RoleCredits>, AppError> {
    queries::track::get_track_role_credits_by_ids(db, ids).await
}

/// The genres behind each selected track, as rows rather than as the rendered column.
pub async fn get_tag_edit_genres(
    db: &DbPool,
    ids: &[i64],
) -> Result<HashMap<i64, GenreList>, AppError> {
    queries::track::get_track_genres_by_ids(db, ids).await
}

/// The release tags behind each selected track, for the dialog's Details tab.
pub async fn get_tag_edit_release_tags(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<ReleaseTagRow>, AppError> {
    queries::album::get_release_tags_for_tracks(db, ids).await
}

/// `artwork_source` is the picked image iff `edit.artwork == ArtworkEdit::Replace`, the writer's
/// `Replace` being a unit variant.
pub async fn apply_tag_edit(
    state: &AppState,
    ids: Vec<i64>,
    edit: TagEdit,
    artwork_source: Option<PathBuf>,
) -> Result<TagEditReport, AppError> {
    let (report, updated_ids) = write_tag_edit(
        &state.db,
        &state.paths.artwork_dir,
        &state.cover_cache,
        &state.self_writes,
        &ids,
        &edit,
        artwork_source.as_deref(),
    )
    .await?;

    if !updated_ids.is_empty() {
        if edit.renames_recording() {
            forget_stored_lyrics(state, &updated_ids).await;
        }

        // Also refreshes Now Playing and the queue: the window task re-reads every record the
        // player holds.
        state.library_changed.bump();
    }

    Ok(report)
}

/// Drop what the lyrics directory answered for each retagged track: the store is keyed on the path,
/// so a month-long miss would outlive the corrected tags. Best-effort, the edit having committed.
async fn forget_stored_lyrics(state: &AppState, ids: &[i64]) {
    let paths = match queries::track::get_track_paths_by_ids(&state.db, ids).await {
        Ok(rows) => rows.into_iter().map(|(_, path)| path).collect(),
        Err(e) => {
            log::debug!("tags: lyrics store kept: {}", describe(&e));
            return;
        }
    };
    if let Err(e) = lyrics::forget_all(state, paths).await {
        log::debug!("tags: lyrics store kept: {}", describe(&e));
    }
}

/// Rewrite each file, re-extract, and land the batch in one write transaction, returning the report
/// and the refreshed ids. Takes no `AppState`, so a `test_pool` and temp files can drive it.
pub(crate) async fn write_tag_edit(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &Arc<SelfWrites>,
    ids: &[i64],
    edit: &TagEdit,
    artwork_source: Option<&Path>,
) -> Result<(TagEditReport, Vec<i64>), AppError> {
    let mut report = TagEditReport::default();

    // lofty rewrites the tag whether or not anything differs, so a reflexive Save would rewrite
    // the whole album and hand the watcher every file back. Here rather than at the door because
    // `library::ratings` calls this directly.
    if edit.is_noop() {
        return Ok((report, Vec::new()));
    }

    let rows = queries::track::get_track_paths_by_ids(db, ids).await?;
    if rows.is_empty() {
        return Ok((report, Vec::new()));
    }

    let artwork_source = artwork_source.map(Path::to_path_buf);

    // The picked cover goes first, so a corrupt pick fails the edit before any file is touched.
    let (files, cached_artwork): (Vec<FileWrite>, Option<String>) = {
        let edit = edit.clone();
        let artwork_dir = artwork_dir.to_path_buf();
        let cover_cache = cover_cache.clone();
        let self_writes = Arc::clone(self_writes);
        tokio::task::spawn_blocking(move || {
            let (picture, cached_artwork) =
                prepare_artwork(&edit, artwork_source.as_deref(), &artwork_dir)?;

            let files = run_write_pass(
                &rows,
                &edit,
                picture.as_ref(),
                &artwork_dir,
                &cover_cache,
                &self_writes,
            );
            Ok::<_, AppError>((files, cached_artwork))
        })
        .await
        .map_err(|e| AppError::metadata("tag write task panicked", e))??
    };

    let updated_ids =
        match run_commit(db, &files, edit, cached_artwork.as_deref(), &mut report).await {
            Ok(ids) => ids,
            Err(e) => {
                // Unmarked so the watcher re-ingests rather than leaving the rows stale.
                let marked: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(&f.path)).collect();
                self_writes.unmark(&marked);
                return Err(e);
            }
        };

    report.updated = updated_ids.len();
    // Here rather than in `apply_tag_edit`, so the rating write-back is covered too and no later
    // `?` drops the report unrecorded.
    report.log_unsupported();
    Ok((report, updated_ids))
}

/// Decode-validate the picked cover, lofty sniffing only 8 bytes, and copy the original into the
/// artwork store. `(None, None)` unless `Replace`. Blocking.
fn prepare_artwork(
    edit: &TagEdit,
    artwork_source: Option<&Path>,
    artwork_dir: &Path,
) -> Result<(Option<lofty::picture::Picture>, Option<String>), AppError> {
    if edit.artwork != ArtworkEdit::Replace {
        return Ok((None, None));
    }
    let source = artwork_source.ok_or_else(|| {
        AppError::metadata_msg("artwork Replace requested without a source image".to_owned())
    })?;
    let picture = cover_embed::cover_picture_from_path(source)?;
    let cached = artwork::cache_image_file(source, artwork_dir);
    Ok((Some(picture), cached))
}

/// One file's result out of the blocking pass. A failure is collected rather than fatal, and
/// flattens to report text where it is read.
struct FileWrite {
    id: i64,
    path: String,
    outcome: Result<(ExtractedMetadata, tag_writer::WriteOutcome), AppError>,
}

fn run_write_pass(
    rows: &[(i64, String)],
    edit: &TagEdit,
    picture: Option<&lofty::picture::Picture>,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &SelfWrites,
) -> Vec<FileWrite> {
    // A Replace discards the re-extract's artwork, `apply_replace_artwork` overwriting every row
    // with the batch's one cover. A rating-only edit can't change the cover either, at the cost of
    // `Keep`'s COALESCE backfill: a track with no `artwork_path` keeps none until the next scan.
    // `Remove` needs the external-cover fallback, and an Edit-Tags `Keep` can sit beside a field
    // that re-homes the track, so both read it.
    let skip_artwork = edit.artwork == ArtworkEdit::Replace || edit.is_rating_only();
    let write_one = |(id, path): &(i64, String)| {
        let p = Path::new(path);
        // Before the write, so the TTL tracks the event it fires, and by the DB `file_path`, the
        // set keying on exact `PathBuf` equality.
        self_writes.mark(p);
        let outcome = tag_writer::apply_to_file(p, edit, picture).and_then(|written| {
            extract_metadata(p, artwork_dir, cover_cache, skip_artwork).map(|meta| (meta, written))
        });
        FileWrite { id: *id, path: path.clone(), outcome }
    };

    // Not the global pool where the build failed: thread starvation would make that one panic on
    // first use too, and it is the pool the UI thread's inline decodes wait on.
    match TAG_WRITE_POOL.as_ref() {
        Some(pool) => pool.install(|| rows.par_iter().map(write_one).collect()),
        None => rows.iter().map(write_one).collect(),
    }
}

/// Land the successful writes, recording failures and unsupported fields into `report`. Answers the
/// refreshed ids.
async fn run_commit(
    db: &DbPool,
    files: &[FileWrite],
    edit: &TagEdit,
    cached_artwork: Option<&str>,
    report: &mut TagEditReport,
) -> Result<Vec<i64>, AppError> {
    let mut written: Vec<(&FileWrite, &tag_writer::WriteOutcome)> = Vec::with_capacity(files.len());
    let mut retagged: Vec<queries::scan::RetaggedFile<'_>> = Vec::with_capacity(files.len());
    for f in files {
        match &f.outcome {
            Ok((meta, outcome)) => {
                retagged.push(queries::scan::RetaggedFile { id: f.id, path: &f.path, meta });
                written.push((f, outcome));
            }
            Err(e) => {
                let reason = describe(e);
                log::warn!("tag write failed for {}: {reason}", f.path);
                report.failures.push((f.path.clone(), reason));
            }
        }
    }

    let retags = queries::scan::commit_retag(db, &retagged, edit, cached_artwork).await?;

    let mut updated_ids: Vec<i64> = Vec::with_capacity(written.len());
    for ((f, outcome), retag) in written.into_iter().zip(retags) {
        match retag {
            queries::scan::Retag::OutsideLibrary => {
                report.failures.push((f.path.clone(), "not in a library folder".to_owned()));
            }
            queries::scan::Retag::Updated => {
                updated_ids.push(f.id);
                if !outcome.unsupported.is_empty() {
                    report.unsupported.push(UnsupportedWrite {
                        path: f.path.clone(),
                        format: outcome.format,
                        fields: outcome.unsupported.clone(),
                    });
                }
            }
        }
    }
    Ok(updated_ids)
}

#[cfg(test)]
#[path = "tests/tags_tests.rs"]
mod tests;
