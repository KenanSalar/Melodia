//! Writes the stars an M4A's row holds into its file where the file holds others or none, once.
//!
//! lofty's `Ilst` merge drops a rating in any shape but the one `rating_tags::write_stars` hands it,
//! the save still succeeding, so a row can hold stars its file lacks. Nothing else repairs that: a
//! star reaches a file only when it is set, and a re-read keeps a row's stars over a file with none.

use crate::library::ratings;
use crate::state::AppState;
use crate::tasks::{TaskSpawner, one_shot};
use melodia_core::error::AppResult;
use melodia_store::database::queries;
use melodia_store::media::ingest::rating_tags;

/// [`one_shot::OnFailure::Retry`]: nothing else writes these stars, and a pass run again rewrites
/// only the files still disagreeing with their rows.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    one_shot::spawn(
        spawner,
        state,
        one_shot::Sweep {
            label: "M4A rating backfill",
            marker: "m4a_ratings_backfilled",
            done: |flags| flags.m4a_ratings_backfilled,
            mark: |flags| flags.m4a_ratings_backfilled = true,
            on_failure: one_shot::OnFailure::Retry,
        },
        |state| async move { backfill(&state).await },
    );
}

/// With the switch off the pass is done without writing: a star set then stays in the database
/// alone, as for every format.
async fn backfill(state: &AppState) -> AppResult<()> {
    if !state.write_ratings_to_tags.get() {
        log::info!("M4A rating backfill: ratings aren't written to tags, so no file was touched");
        return Ok(());
    }

    let tally = write_stale_files(state).await?;
    log::info!(
        "M4A rating backfill: {} file(s) took their stars, {} failed",
        tally.written,
        tally.failed
    );
    one_shot::require_all_reached(tally.out_of_reach, "M4A(s)")
}

async fn write_stale_files(state: &AppState) -> AppResult<one_shot::Rewrites> {
    let mut tally = one_shot::Rewrites::default();
    let mut pages = one_shot::Pages::default();
    while let Some(page) = pages
        .next_page(|after_id| {
            queries::track::get_rated_mp4_paths_after(&state.db, after_id, one_shot::PAGE_ROWS)
        })
        .await?
    {
        let files =
            one_shot::read_page(page, |rows| one_shot::read_files(rows, rating_tags::read_stars))
                .await?;
        tally
            .rewrite_each(&state.shutdown_token, files, |id, file_stars| {
                write_if_stale(state, id, file_stars.unwrap_or(0))
            })
            .await?;
    }
    Ok(tally)
}

/// The row is read beside the write rather than with its page, so a star clicked meanwhile is the
/// one compared, and a row cleared meanwhile is left to the write-back its clear queued.
async fn write_if_stale(
    state: &AppState,
    id: i64,
    file_stars: i32,
) -> AppResult<one_shot::Rewrite> {
    let Some(rating) = queries::track::get_rating(&state.db, id).await? else {
        return Ok(one_shot::Rewrite::Skipped);
    };
    if rating == 0 || rating == file_stars {
        return Ok(one_shot::Rewrite::Skipped);
    }

    let written = ratings::write_rating_to_files(
        &state.db,
        &state.paths.artwork_dir,
        &state.cover_cache,
        &state.self_writes,
        &[id],
        rating,
    )
    .await?;
    Ok(if written == 0 { one_shot::Rewrite::Failed } else { one_shot::Rewrite::Written })
}
