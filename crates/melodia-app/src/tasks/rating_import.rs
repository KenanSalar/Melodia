//! Reads the ratings already sitting in this library's files into the rows that describe them,
//! once.
//!
//! The scan learned to read a rating later than it learned to read everything else, and
//! `scanner::track_is_current` skips a file whose size and mtime haven't moved — so a library
//! scanned before that lands unrated and stays unrated however many times it is rescanned. Every
//! star a user set in Windows, `MusicBee` or `foobar2000` before installing Melodia is sitting in
//! those files, invisible.
//!
//! **Not an `SQLx` migration**, for [`super::artwork_renormalize`]'s reason: it is a slow pass
//! over files, and a migration failure is fatal at boot.
//!
//! Only unrated rows are read, so the sweep can never overwrite a star set here. It is still
//! one-shot rather than repeated, because `rating = 0` cannot tell an untouched row from one the
//! user deliberately cleared — running it twice would resurrect the second.

use std::collections::HashMap;
use std::path::Path;

use lofty::file::TaggedFileExt;

use crate::state::{AppState, Signal};
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::{AppError, AppResult};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::media::ingest::{metadata, rating_tags};

/// Tracks whose paths are held in memory at once, and whose tags are parsed in one fan-out.
///
/// The predicate selects the whole library on the run that matters, so the page size is what keeps
/// this pass off the RSS budget. Wide enough that the per-page round trip is noise beside the tag
/// parses it feeds, and narrow enough to hand the global Rayon pool back between pages: the boot
/// this runs on is the one the first-launch reconcile scan wants that pool for too.
const PAGE_ROWS: i64 = 2_000;

/// Run the import unless this install has already had one.
///
/// [`one_shot::OnFailure::Retry`], which is where this parts company with
/// [`super::artwork_renormalize`]'s otherwise identical shape. Two of the three ways this can fail
/// write no rows at all, and the marker is the only gate there is, so recording one would put
/// every rating in this library's files out of reach for the life of the install. A renormalize
/// that half-ran is repaired by the next scan; this is not repaired by anything.
///
/// A half-imported library needs no marker either: the pass only ever reads unrated rows, so the
/// retry picks up exactly where it stopped.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    one_shot::spawn(
        spawner,
        state,
        one_shot::Sweep {
            label: "Rating import",
            marker: "ratings_imported_from_tags",
            done: |flags| flags.ratings_imported_from_tags,
            mark: |flags| flags.ratings_imported_from_tags = true,
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

    // Nothing else will say so. This lands minutes into a session on a real library, by which
    // time every mounted list is painting the zero these rows carried at fetch time, and the two
    // views that retain their rows keep it until something unrelated refetches. Once at the end
    // rather than per page: each bump costs the mounted section a whole re-query.
    library_changed.bump();

    log::info!("Imported {imported} rating(s) from file tags");
    Ok(())
}

/// Walk the unrated rows a page at a time, answering with how many took a rating out of a file.
///
/// Keyset rather than offset because each page's write takes its own rows out of the predicate:
/// a window counted from the start would step over as many rows as it had just rated. `page_rows`
/// is a parameter so a test can reach a second page at all; the caller spells [`PAGE_ROWS`].
async fn import_into(db: &DbPool, page_rows: i64) -> AppResult<usize> {
    let mut after_id = 0;
    let mut imported = 0;

    loop {
        let page = queries::track::get_unrated_track_paths_after(db, after_id, page_rows).await?;
        let Some(last_id) = page.last().map(|(id, _)| *id) else {
            break;
        };
        after_id = last_id;

        let found = tokio::task::spawn_blocking(move || read_each(&page))
            .await
            .map_err(|e| AppError::scanner("Rating import task panicked", e))?;
        if found.is_empty() {
            continue;
        }

        // Grouped by value so a page is at most five UPDATEs, each already chunked against the
        // bind cap by `set_rating`.
        let mut by_rating: HashMap<i32, Vec<i64>> = HashMap::new();
        for (id, rating) in &found {
            by_rating.entry(*rating).or_default().push(*id);
        }
        for (rating, ids) in &by_rating {
            queries::track::set_rating(db, ids, *rating).await?;
        }
        imported += found.len();
    }

    Ok(imported)
}

/// **Blocking** — one tag parse per row, fanned out the way `retroactive_hash` fans out its
/// hashes. The rating is a text item, so this asks for neither half lofty would otherwise do on
/// the way to it: copying the embedded pictures out, and the frame scan a headerless VBR MP3's
/// duration costs.
fn read_each(unrated: &[(i64, String)]) -> Vec<(i64, i32)> {
    use rayon::prelude::*;

    unrated
        .par_iter()
        .filter_map(|(id, path)| {
            let tagged = metadata::read_tags(Path::new(path), metadata::TagScope::TagsOnly).ok()?;
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
            rating_tags::stars_from_tag(tag).map(|stars| (*id, stars))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/rating_import_tests.rs"]
mod tests;
