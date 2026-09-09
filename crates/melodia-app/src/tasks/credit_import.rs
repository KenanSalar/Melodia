//! Reads the multi-value artist tags already sitting in this library's files into the credit
//! tables, once.
//!
//! The migration that added those tables seeded one credit per track from `tracks.artist_id`,
//! which is right for a solo artist and is all a library indexed before them can know. Anything
//! more is in the files, and `scanner::track_is_current` skips a file whose size and mtime haven't
//! moved — so a rescan will never reach it, however many times it runs.
//!
//! Two things need fixing on such a row, not one. The credit rows are the obvious half. The other
//! is `tracks.artist_id`, which the old ingest resolved from the *whole* credit line: a library
//! scanned before this feature has an `artists` row literally named "X feat. Y", and every track
//! crediting them points at it. Re-pointing the FK is what makes those rows prunable.
//!
//! **Not an `SQLx` migration**, for [`super::rating_import`]'s reason: it is a slow pass over
//! files, and a migration failure is fatal at boot.

use std::path::Path;

use crate::state::{AppState, Signal};
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::error::{AppError, AppResult};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::media::ingest::metadata;

/// Tracks whose paths are held in memory at once, and whose tags are parsed in one fan-out.
/// [`super::rating_import::PAGE_ROWS`]' reasoning, and deliberately the same number: the two
/// passes do the same shape of work and can run on the same boot.
const PAGE_ROWS: i64 = 2_000;

/// The sentinel artist, for the one argument [`queries::scan::replace_track_credits`] only reads
/// over an empty credit. This pass writes none, having filtered them out.
const UNKNOWN_ARTIST_ID: i64 = 1;

/// Run the import unless this install has already had one.
///
/// [`one_shot::OnFailure::Retry`], like the rating import and for the same reason: nothing else
/// repairs what a half-run pass left, so a marker recorded over a failure puts every multi-artist
/// credit in this library out of reach for the life of the install. A half-imported library needs
/// no marker either — a track this pass rewrote now carries two credit rows, which is exactly what
/// the work-list predicate excludes, so the retry resumes where it stopped.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    one_shot::spawn(
        spawner,
        state,
        one_shot::Sweep {
            label: "Artist credit import",
            marker: "artist_credits_imported",
            done: |flags| flags.artist_credits_imported,
            mark: |flags| flags.artist_credits_imported = true,
            on_failure: one_shot::OnFailure::Retry,
        },
        |state| async move { import(&state.db, &state.library_changed).await },
    );
}

async fn import(db: &DbPool, library_changed: &Signal) -> AppResult<()> {
    let imported = import_into(db, PAGE_ROWS).await?;
    if imported == 0 {
        return Ok(());
    }

    library_changed.bump();
    log::info!("Imported {imported} multi-artist credit(s) from file tags");
    Ok(())
}

/// Walk the single-credit rows a page at a time, answering with how many gained a real credit.
///
/// Keyset paging, and here it is only for the page size: unlike the rating import, a rewritten row
/// *does* leave the predicate, so an offset window would step over rows it had just written.
/// `page_rows` is a parameter so a test can reach a second page at all.
async fn import_into(db: &DbPool, page_rows: i64) -> AppResult<usize> {
    let mut after_id = 0;
    let mut imported = 0;

    loop {
        let page =
            queries::track::get_single_credit_track_paths_after(db, after_id, page_rows).await?;
        let Some(last_id) = page.last().map(|(id, _)| *id) else {
            break;
        };
        after_id = last_id;

        let found = tokio::task::spawn_blocking(move || read_each(&page))
            .await
            .map_err(|e| AppError::scanner("Credit import task panicked", e))?;
        if found.is_empty() {
            continue;
        }

        imported += write_page(db, &found).await?;
    }

    if imported > 0 {
        // The `artists` rows named for a whole credit line are unreferenced now, and nothing else
        // will notice: the row's own FK was the last thing pointing at them. Once at the end
        // rather than per page, the sweep being the only writer for its duration.
        let mut tx = db.write().begin().await?;
        queries::scan::prune_orphans(&mut tx).await?;
        tx.commit().await?;
    }

    Ok(imported)
}

/// Land one page's credits in a single transaction, re-pointing each row's `artist_id` at the
/// name that is actually first.
async fn write_page(db: &DbPool, found: &[(i64, ArtistCredit)]) -> AppResult<usize> {
    let mut tx = db.write().begin().await?;
    for (id, credit) in found {
        let primary =
            queries::scan::upsert_artist(&mut tx, credit.primary_name(), UNKNOWN_ARTIST_ID).await?;
        queries::scan::replace_track_credits(&mut tx, *id, credit, primary).await?;
        sqlx::query("UPDATE tracks SET artist_id = ?, artist = ? WHERE id = ?")
            .bind(primary)
            .bind(credit.line())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(found.len())
}

/// **Blocking** — one tag parse per row, fanned out the way the rating import fans out its own.
///
/// Only a credit naming more than one artist is worth a write: a single name is what the row
/// already says, and rewriting it would cost the same transaction to change nothing.
fn read_each(page: &[(i64, String)]) -> Vec<(i64, ArtistCredit)> {
    use rayon::prelude::*;

    page.par_iter()
        .filter_map(|(id, path)| {
            let (artist, _) = metadata::read_credits(Path::new(path)).ok()?;
            (artist.artists().len() > 1).then_some((*id, artist))
        })
        .collect()
}
