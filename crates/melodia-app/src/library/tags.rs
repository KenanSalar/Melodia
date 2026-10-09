//! Edit Track Information orchestrator: turn a [`TagEdit`] plus a set of track
//! ids into rewritten files, a byte-consistent DB, and a refreshed player.
//!
//! Everything *after* the tag write reuses the scan pipeline: re-extract the
//! file ([`extract_metadata`]), then [`queries::scan::commit_retag`] resolves the
//! artist/album/genre ids and refreshes the row through the scan's own
//! `update_track_metadata` — which recomputes `file_hash` / `file_size` /
//! `date_modified` / `sort_key` / `duration_ms` and lets the existing FTS /
//! stats triggers do the reindex and rollups with no new SQL.
//!
//! Do **not** shortcut this into a hand-built `UPDATE` from the form values: a
//! tag write rewrites the file's bytes, so hash/size/mtime all change, and a
//! fresh `date_modified` beside a stale `file_hash` is the one state
//! `scanner::track_is_current` reads as current forever. The re-extract is what
//! keeps those three honest.

use std::collections::{HashMap, HashSet};
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
use melodia_core::entities::track::{TagEditRow, TrackSummary};
use melodia_core::error::AppError;
use melodia_core::error::describe;
use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::database::{DbPool, queries};
use melodia_store::media::ingest::metadata::extract_metadata;
use melodia_store::media::ingest::{cover_embed, tag_writer};

/// Width cap for the tag-write fan-out. The MP4 save clones the embedded cover,
/// so an unbounded `par_iter` would hold `num_cpus × (image + its clone)`
/// resident at once — a memory regression in a project that exists because of
/// them. Tag writing is I/O-bound, so extra width buys nothing anyway.
const TAG_WRITE_THREADS: usize = 4;

/// The fan-out pool itself, built once. A rating write reaches here on a single click and usually
/// carries one file, where building and joining four OS threads per call is most of the work, and
/// the thread churn lands on the same glibc arena the audio callback allocates from.
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

/// Outcome of applying a [`TagEdit`] to a batch of tracks. Partial success is
/// reported, never rolled back — a read-only file or an unsupported container
/// fails only its own row.
#[derive(Debug, Default)]
pub struct TagEditReport {
    pub updated: usize,
    /// `(file, error)` for files whose write or re-extract failed.
    pub failures: Vec<(String, String)>,
    /// Files whose container had no key for part of the edit — a rare safety
    /// net, since all three primary tag types map every exposed field.
    pub unsupported: Vec<UnsupportedWrite>,
}

/// One file's worth of "that field did not fit in this container".
///
/// The format rides along because it is the *reason*, and the toast has no other way to reach it:
/// a path does not say what the container was, and deriving it from the extension would re-answer
/// a question the writer already answered off the file's own header.
#[derive(Debug, Clone)]
pub struct UnsupportedWrite {
    pub path: String,
    /// `None` for a container lofty grew after this build, which costs the toast the name and
    /// puts it back on the count.
    pub format: Option<&'static str>,
    pub fields: Vec<TagField>,
}

impl TagEditReport {
    /// The one line a toast can draw, when every file failed the same way.
    ///
    /// `None` where a batch spans containers or disagrees about which fields fell out, because a
    /// single sentence would then have to lie about one of them; the caller falls back to a count.
    /// A batch is normally one album in one format, so this is the usual answer rather than the
    /// lucky one.
    #[must_use]
    pub fn unsupported_agreement(&self) -> Option<(&'static str, &[TagField])> {
        let (first, rest) = self.unsupported.split_first()?;
        let format = first.format?;
        let agreed =
            rest.iter().all(|other| other.format == first.format && other.fields == first.fields);
        agreed.then_some((format, &first.fields))
    }

    /// Record what the batch could not write, folded the way the toast folds it.
    ///
    /// Per file would be the obvious shape and is the wrong one here: a batch is however many
    /// tracks the user selected, the log rotates at a size, and nobody reads five thousand lines
    /// saying the same thing.
    ///
    /// One path still survives per group, because a count alone cannot be chased: it names a file
    /// to open and the rest are the same edit against the same container.
    ///
    /// A commit that fails returns `Err` before this and takes the report with it, so a
    /// rolled-back edit records no unsupported field. That is the right way round: the rows went
    /// back, the watcher has the files again, and which key did not fit is not what went wrong
    /// there.
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
///
/// Sits here rather than being called straight off `queries::track` by the
/// dialog's own wiring: the UI layer reaches the database through this module,
/// and [`apply_tag_edit`] below is the write half of the same feature.
pub async fn get_tag_edit_rows(state: &AppState, ids: &[i64]) -> Result<Vec<TagEditRow>, AppError> {
    queries::track::get_tag_edit_rows_by_ids(&state.db, ids).await
}

/// A file's lyrics tag, read off the file rather than the database.
///
/// `library::lyrics` is what is left of its callers, handing what comes back to an LRC parser
/// because this tag is routinely filled with a timed sheet. The string is whatever the tag held;
/// nothing here judges what is in it. Blocking — the caller owns the `spawn_blocking`, having a
/// runtime handle in hand where this does not.
///
/// The Edit-Tags dialog took this once and takes [`read_lyrics_and_credits`] now, wanting the
/// artist credit off the same open. Which is why this stays: `library::lyrics` wants the lyrics
/// and nothing else, and a second parse of the whole tag is not free.
pub fn read_lyrics(path: &Path) -> Result<Option<String>, AppError> {
    tag_writer::read_lyrics(path)
}

/// Everything the dialog's single-selection path reads out of the file itself: the lyrics tag and
/// the two artist credits, off one open.
///
/// The credit comes from the file rather than the database, and the asymmetry is the point: the
/// file is what the credit *is*, and a track whose join rows were seeded from `artist_id` alone
/// (every row on a library that predates the credit tables) would otherwise open showing one name
/// for a credit the file spells with three. A multi-track selection reads
/// [`queries::track::get_track_credits_by_ids`] instead, N file reads on the open path being
/// exactly what the `TagEditRow` projection exists to avoid.
///
/// Blocking; the caller owns the `spawn_blocking`.
pub fn read_lyrics_and_credits(
    path: &Path,
) -> Result<(Option<String>, (ArtistCredit, ArtistCredit)), AppError> {
    tag_writer::read_lyrics_and_credits(path)
}

/// The credit behind each selected track, for the dialog's multi-selection path.
pub async fn get_tag_edit_credits(
    state: &AppState,
    ids: &[i64],
) -> Result<HashMap<i64, (ArtistCredit, ArtistCredit)>, AppError> {
    queries::track::get_track_credits_by_ids(&state.db, ids).await
}

/// The role credits behind each selected track.
///
/// From the database for a single track as well as a selection, unlike [`get_tag_edit_credits`]'s
/// caller, which reads one track's artist credit off the file. There is no second read to share
/// here — the lyrics tab's open covers the artist tags and the lyrics, not these — so the row,
/// re-ingested from the file on every scan and every save, is the cheaper of two right answers.
pub async fn get_tag_edit_role_credits(
    state: &AppState,
    ids: &[i64],
) -> Result<HashMap<i64, RoleCredits>, AppError> {
    queries::track::get_track_role_credits_by_ids(&state.db, ids).await
}

/// The genres behind each selected track, as rows rather than as the rendered column.
pub async fn get_tag_edit_genres(
    state: &AppState,
    ids: &[i64],
) -> Result<HashMap<i64, GenreList>, AppError> {
    queries::track::get_track_genres_by_ids(&state.db, ids).await
}

/// The release tags behind each selected track, for the dialog's Details tab.
pub async fn get_tag_edit_release_tags(
    state: &AppState,
    ids: &[i64],
) -> Result<Vec<ReleaseTagRow>, AppError> {
    queries::album::get_release_tags_for_tracks(&state.db, ids).await
}

/// Apply `edit` to `ids`, then refresh the player's cached summaries and bump
/// `library_changed` so the visibility-gated list views re-fetch.
///
/// `artwork_source` is `Some(path)` iff `edit.artwork == ArtworkEdit::Replace`
/// (the picked image file): the writer's `Replace` is a unit variant, so the
/// path rides alongside the edit and the orchestrator owns it (it needs it for
/// [`artwork::cache_image_file`] anyway).
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
        // Ahead of the resync, which is the one step here that can still fail: the files are
        // written and the batch is committed, so a read error below would otherwise leave the
        // stale answer standing over something that had nothing to do with it.
        if edit.renames_recording() {
            forget_stored_lyrics(state, &updated_ids).await;
        }

        // Overwrite any queued / currently-playing summary with its fresh copy
        // so the Now-Playing bar, Queue Sheet and Up Next stop showing old tags.
        // Only pay for the refetch + resync when the player actually references
        // an edited track — the uncommon case. `sync_track_summaries` no-ops
        // otherwise, but the fetch + `HashMap` build run before its own check,
        // so gate them on the same cheap membership probe.
        let touched: HashSet<i64> = updated_ids.iter().copied().collect();
        if melodia_engine::player::engine::state::any_tracked(&state.player_state, |id| {
            touched.contains(&id)
        }) {
            let fresh = queries::track::get_track_summaries_by_ids(&state.db, &updated_ids).await?;
            let map: HashMap<i64, TrackSummary> = fresh.into_iter().map(|t| (t.id, t)).collect();
            melodia_engine::player::engine::state::sync_track_summaries(
                &state.player_state,
                &state.sinks,
                &map,
            );
        }

        state.library_changed.bump();
    }

    Ok(report)
}

/// Drop what the lyrics directory answered for each retagged track.
///
/// The store is keyed on the file's path, so nothing here expires when the tags it was matched off
/// change, and a miss stands for a month. That is how a track keeps showing no lyrics after the
/// artist that lost them has been corrected.
///
/// Best-effort: the edit has already committed, so a store entry that would not clear is a line in
/// the log rather than a save to fail.
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

/// The testable core: rewrite each file's tags, re-extract, and land the batch
/// in one write transaction. Takes only the pieces it needs (no `AppState`, no
/// player, no watch channel) so it can be exercised with a `test_pool` and real
/// temp files. Returns the report plus the ids whose rows were refreshed (for
/// the caller's player resync).
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

    // lofty rewrites the tag whether or not anything differs, so an edit carrying no change must
    // stop short of the write pass: a reflexive Save on a 200-track album would otherwise rewrite
    // all 200 and hand the watcher every one of them back. Here rather than at the door because
    // `library::ratings` reaches this function directly.
    if edit.is_noop() {
        return Ok((report, Vec::new()));
    }

    let rows = queries::track::get_track_paths_by_ids(db, ids).await?;
    if rows.is_empty() {
        return Ok((report, Vec::new()));
    }

    // Owned copy so the picked-cover decode can move into the blocking task.
    let artwork_source = artwork_source.map(Path::to_path_buf);

    // Blocking, width-capped file write + re-extract pass. The picked cover is
    // decoded + cached *inside* the task too: `cover_picture_from_path` (image
    // decode) and `cache_image_file` (file read + copy) are blocking I/O + CPU,
    // so they belong off the async worker. The cover is handled FIRST — a corrupt
    // pick fails the whole edit (via `?`) before any file is touched.
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
                // Only on the (rare) tx failure: unmark every path we marked before
                // writing, so the watcher re-ingests instead of leaving the DB
                // permanently stale. Built here, not on the happy path, to avoid N
                // `PathBuf` allocations per successful commit.
                let marked: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(&f.path)).collect();
                self_writes.unmark(&marked);
                return Err(e);
            }
        };

    report.updated = updated_ids.len();
    // Here rather than in `apply_tag_edit`, so the rating write-back is covered too and so no
    // later `?` can drop the report before it is recorded.
    report.log_unsupported();
    Ok((report, updated_ids))
}

/// Decode + validate the picked cover and copy it into the content-addressed
/// artwork dir. `Keep` / `Remove` need neither, so return `(None, None)`.
///
/// Blocking (image decode + file copy) — call from inside the `spawn_blocking`
/// write pass. `cover_picture_from_path` decode-validates (lofty only sniffs 8
/// bytes) and re-encodes non-JPEG/PNG; `cache_image_file` copies the original
/// bytes for the DB `artwork_path`. Run FIRST — a corrupt pick fails the whole
/// edit here (via `?`), before any file is touched.
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

/// Per-file result carried out of the blocking pass. `Ok((meta, unsupported))`
/// on a successful write + re-extract; a write or re-extract failure is
/// collected here rather than being fatal, and flattens to report text at the
/// point it is read.
struct FileWrite {
    id: i64,
    path: String,
    outcome: Result<(ExtractedMetadata, tag_writer::WriteOutcome), AppError>,
}

/// Rewrite each file's tags on a bounded Rayon pool, then re-extract. Mirrors
/// `scanner::scan_files_parallel`, but the pool is capped (see
/// [`TAG_WRITE_THREADS`]).
fn run_write_pass(
    rows: &[(i64, String)],
    edit: &TagEdit,
    picture: Option<&lofty::picture::Picture>,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &SelfWrites,
) -> Vec<FileWrite> {
    // On a Replace the re-extract's artwork is discarded: `apply_replace_artwork`
    // overwrites every row (and album) with the batch's single `cached_artwork`,
    // so reading the just-embedded cover back (a full decode + a redundant BLAKE3
    // pass + an artwork-dir write, per track, un-memoized) is pure waste.
    //
    // A rating-only edit skips it for a different reason and gives something up
    // to do so: nothing it writes can change the cover, so parsing the embedded
    // picture back out costs a decode per track for an answer already on the row.
    // What that gives up is `Keep`'s COALESCE, which also backfills a missing
    // `artwork_path`, so a track that has none keeps none until the next scan.
    // `Remove` still needs the external-cover fallback, and a real Edit-Tags
    // `Keep` can arrive beside a field that re-homes the track, so both stay on
    // the full path.
    let skip_artwork = edit.artwork == ArtworkEdit::Replace || edit.is_rating_only();
    let write_one = |(id, path): &(i64, String)| {
        let p = Path::new(path);
        // Mark BEFORE the write, per file, so the TTL clock tracks the
        // event this write is about to fire — and with the DB `file_path`
        // (the set keys on exact `PathBuf` equality).
        self_writes.mark(p);
        let outcome = tag_writer::apply_to_file(p, edit, picture).and_then(|written| {
            extract_metadata(p, artwork_dir, cover_cache, skip_artwork).map(|meta| (meta, written))
        });
        FileWrite { id: *id, path: path.clone(), outcome }
    };

    // Sequentially, rather than on the global pool, where the build failed. Thread starvation is
    // the only realistic way to arrive here, and the global pool panics on first use if that kept
    // it from building too. Where it did build, it is the pool the UI thread's inline decodes wait
    // on.
    match TAG_WRITE_POOL.as_ref() {
        Some(pool) => pool.install(|| rows.par_iter().map(write_one).collect()),
        None => rows.iter().map(write_one).collect(),
    }
}

/// Land the successful writes through [`queries::scan::commit_retag`], recording per-file failures
/// and unsupported fields into `report`. Answers the ids whose rows were refreshed.
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
