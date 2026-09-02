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

use crate::database::queries;
use crate::error::{AppError, AppResult};
use crate::media::{metadata, rating_tags};
use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};

/// Run the import unless this install has already had one.
///
/// [`OnFailure::Retry`], which is where this parts company with [`super::artwork_renormalize`]'s
/// otherwise identical shape. Two of the three ways this can fail write no rows at all, and the
/// marker is the only gate there is, so recording one would put every rating in this library's
/// files out of reach for the life of the install. A renormalize that half-ran is repaired by the
/// next scan; this is not repaired by anything.
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
        |state| async move { import(&state).await },
    );
}

async fn import(state: &AppState) -> AppResult<()> {
    let unrated = queries::track::get_unrated_track_paths(&state.db).await?;
    if unrated.is_empty() {
        return Ok(());
    }

    let found = tokio::task::spawn_blocking(move || read_each(&unrated))
        .await
        .map_err(|e| AppError::scanner("Rating import task panicked", e))?;
    if found.is_empty() {
        return Ok(());
    }

    // Grouped by value so the whole pass is at most five UPDATEs, each already chunked against
    // the bind cap by `set_rating`.
    let mut by_rating: HashMap<i32, Vec<i64>> = HashMap::new();
    for (id, rating) in &found {
        by_rating.entry(*rating).or_default().push(*id);
    }
    for (rating, ids) in &by_rating {
        queries::track::set_rating(&state.db, ids, *rating).await?;
    }

    // Nothing else will say so. This lands minutes into a session on a real library, by which
    // time every mounted list is painting the zero these rows carried at fetch time, and the two
    // views that retain their rows keep it until something unrelated refetches.
    state.library_changed.bump();

    log::info!("Imported {} rating(s) from file tags", found.len());
    Ok(())
}

/// **Blocking** — one tag parse per row, fanned out the way `retroactive_hash` fans out its
/// hashes. Artwork is skipped: the rating is a text item, and decoding a cover to read one is
/// the expensive half of a parse for no answer.
fn read_each(unrated: &[(i64, String)]) -> Vec<(i64, i32)> {
    use rayon::prelude::*;

    unrated
        .par_iter()
        .filter_map(|(id, path)| {
            let tagged = metadata::read_tags(Path::new(path), true).ok()?;
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
            rating_tags::stars_from_tag(tag).map(|stars| (*id, stars))
        })
        .collect()
}
