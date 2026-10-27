//! Reads the ratings already in this library's files into their rows, once.
//!
//! `scanner::track_is_current` skips an unchanged file, so a library scanned before ratings were
//! read stays unrated however often it is rescanned, every star another player wrote sitting
//! unread. **Not an `SQLx` migration**, for [`super::artwork_renormalize`]'s reason. Only unrated
//! rows are read, and only once, since `rating = 0` can't tell an untouched row from a cleared one.

use std::collections::HashMap;
use std::path::Path;

use lofty::file::TaggedFileExt;

use crate::state::{AppState, Signal};
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::{AppError, AppResult};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::media::ingest::scan_pool::ScanPool;
use melodia_store::media::ingest::{metadata, rating_tags};

/// Rows held and parsed per page. The whole library matches on the run that matters, so this is
/// what keeps the pass off the RSS budget.
const PAGE_ROWS: i64 = 2_000;

/// [`one_shot::OnFailure::Retry`]: two of the three failures write no rows, and a marker over them
/// would put every rating in the files out of reach, nothing else repairing it. A half-run import
/// resumes cleanly, the pass reading only what is still unrated.
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

    // Nothing else repaints the zeros mounted lists fetched. Once rather than per page, each bump
    // costing the mounted section a whole re-query.
    library_changed.bump();

    log::info!("Imported {imported} rating(s) from file tags");
    Ok(())
}

/// Answers how many rows took a rating out of a file. Keyset rather than offset, each page's write
/// taking its rows out of the predicate; `page_rows` is a parameter so a test can reach a second
/// page.
async fn import_into(db: &DbPool, page_rows: i64) -> AppResult<usize> {
    let mut after_id = 0;
    let mut imported = 0;

    loop {
        let page = queries::track::get_unrated_track_paths_after(db, after_id, page_rows).await?;
        let Some(last_id) = page.last().map(|(id, _)| *id) else {
            break;
        };
        after_id = last_id;

        let found = tokio::task::spawn_blocking(move || {
            ScanPool::for_files(page.len()).install(|| read_each(&page))
        })
        .await
        .map_err(|e| AppError::scanner("Rating import task panicked", e))?;
        if found.is_empty() {
            continue;
        }

        // Grouped by value: at most five UPDATEs a page, `set_rating` chunking each.
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

/// **Blocking**, fanned out like `retroactive_hash`. The rating is a text item, so this skips the
/// picture copies and the VBR frame scan lofty would otherwise do on the way.
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
