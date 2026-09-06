//! Tests for the first-launch sweep that reads ratings *out* of files, against real fixtures
//! copied out of `test-assets/` into a `TempDir`. Never write to the checked-in asset.
//!
//! The same data as `rating_writeback`, flowing the other way: a library already rated in another
//! player arrives with its stars in the tags and nowhere else. What matters is that the sweep
//! reports only what it actually found, since every id it returns becomes a row update, and a
//! track it wrongly reports as zero-starred is indistinguishable from one the user cleared.

use std::path::PathBuf;

use tempfile::TempDir;

use super::*;
use melodia_core::entities::tags::{FieldEdit, TagEdit};
use melodia_store::media::ingest::tag_writer;
use melodia_testkit::ASSETS_DIR;

/// Stage a fixture and, when `stars` is given, put a rating in it through the production writer.
fn staged(tmp: &TempDir, name: &str, stars: Option<i32>) -> Result<String, AppError> {
    let dst = tmp.path().join(name);
    std::fs::copy(PathBuf::from(ASSETS_DIR).join("silence.flac"), &dst)?;
    if let Some(stars) = stars {
        let edit = TagEdit {
            rating: FieldEdit::Set(stars),
            ..TagEdit::default()
        };
        tag_writer::apply_to_file(&dst, &edit, None)?;
    }
    Ok(dst.to_string_lossy().into_owned())
}

/// The three answers a row can give, in one pass, because the sweep is a `filter_map` over all of
/// them at once: a rated file, an unrated one, and a path whose file is gone. Only the first may
/// appear in the result, and the other two must not take the pass down with them.
#[test]
fn the_sweep_reports_only_the_ratings_it_actually_found() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let rated = staged(&tmp, "rated.flac", Some(4))?;
    let unrated = staged(&tmp, "unrated.flac", None)?;
    let missing = tmp.path().join("never-existed.flac").to_string_lossy().into_owned();

    let found = read_each(&[(1, rated), (2, unrated), (3, missing)]);

    assert_eq!(
        found,
        vec![(1, 4)],
        "an unrated file and an unreadable one are both absences, not zero-star ratings"
    );
    Ok(())
}

/// Every star the writer can put in a file has to come back out as itself. A sweep that rounds or
/// clamps on the way in would rewrite ratings the user set in another player.
#[test]
fn every_rating_survives_the_round_trip_out_of_the_file() -> Result<(), AppError> {
    let tmp = TempDir::new()?;

    for stars in 1..=rating_tags::MAX_STARS {
        let path = staged(&tmp, &format!("stars-{stars}.flac"), Some(stars))?;
        assert_eq!(
            read_each(&[(stars.into(), path)]),
            vec![(i64::from(stars), stars)],
            "{stars} star(s) must read back as {stars}"
        );
    }
    Ok(())
}

// --- The page walk ----------------------------------------------------------
//
// `read_each` above is the per-file half; this is the loop around it, and it is the half that
// decides which files get read at all. One-shot and `OnFailure::Retry`, so a row this walk steps
// over keeps its stars in the file and nowhere else for the life of the install.

use melodia_store::database::queries::fixtures::insert_test_track;

/// A row per entry, each pointing at a real fixture carrying `stars` (or none), in id order.
///
/// The folder goes in first because `insert_test_track` hard-codes `folder_id: 1`.
async fn staged_library(
    db: &DbPool,
    tmp: &TempDir,
    stars: &[Option<i32>],
) -> Result<Vec<i64>, AppError> {
    queries::folder::insert_folder(db, &tmp.path().to_string_lossy(), true).await?;

    let mut ids = Vec::with_capacity(stars.len());
    for (n, stars) in stars.iter().enumerate() {
        let path = staged(tmp, &format!("track{n}.flac"), *stars)?;
        let title = format!("Track {n}");
        ids.push(insert_test_track(db, &path, &title, "Artist", "Album", "Rock").await?);
    }
    Ok(ids)
}

async fn ratings_by_id(db: &DbPool) -> Result<Vec<(i64, i32)>, AppError> {
    let mut rows: Vec<(i64, i32)> = queries::track::get_all_tracks_for_list(db)
        .await?
        .into_iter()
        .map(|row| (row.id, row.rating))
        .collect();
    rows.sort_unstable();
    Ok(rows)
}

/// One row per page, so the second page is asked for after the first has been written. An
/// `OFFSET` walk starts the second page past the row the first just took out of the predicate,
/// which skips every other file in the library and reports a plausible count doing it.
#[tokio::test]
async fn a_page_that_rates_its_rows_does_not_step_over_the_next_page() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = DbPool::test_pool().await?;
    let ids = staged_library(&db, &tmp, &[Some(1), Some(2), Some(3)]).await?;

    let imported = import_into(&db, 1).await?;

    assert_eq!(imported, 3);
    assert_eq!(ratings_by_id(&db).await?, vec![(ids[0], 1), (ids[1], 2), (ids[2], 3)]);
    Ok(())
}

/// A page whose files carry no rating is not the end of the library. The `continue` that skips
/// the write and the `break` that ends the walk sit four lines apart, and confusing them strands
/// every rated file behind the first unrated one.
#[tokio::test]
async fn a_page_holding_no_ratings_does_not_end_the_walk() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = DbPool::test_pool().await?;
    let ids = staged_library(&db, &tmp, &[None, Some(4)]).await?;

    import_into(&db, 1).await?;

    assert_eq!(ratings_by_id(&db).await?, vec![(ids[0], 0), (ids[1], 4)]);
    Ok(())
}

/// A page becomes one `UPDATE` per distinct star count, so the grouping is what keeps a rating
/// with the row it came from. Interleaved rather than blocked, so a version writing the page's
/// first rating to the whole page reads differently from one that does not.
#[tokio::test]
async fn each_row_takes_the_rating_out_of_its_own_file() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = DbPool::test_pool().await?;
    let ids = staged_library(&db, &tmp, &[Some(5), Some(2), Some(5), Some(2)]).await?;

    import_into(&db, 100).await?;

    assert_eq!(ratings_by_id(&db).await?, vec![(ids[0], 5), (ids[1], 2), (ids[2], 5), (ids[3], 2)]);
    Ok(())
}

/// The bump costs every mounted section a whole re-query, and a library that was already rated
/// reaches the end of the walk having written nothing.
#[tokio::test]
async fn a_sweep_that_rated_nothing_wakes_nobody() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = DbPool::test_pool().await?;
    staged_library(&db, &tmp, &[None, None]).await?;
    let library_changed = Signal::new();
    let subscriber = library_changed.subscribe();

    import(&db, &library_changed).await?;

    assert!(!matches!(subscriber.has_changed(), Ok(true)));
    Ok(())
}

/// The other half, and the reason the bump is there at all: the rows were fetched before the
/// sweep ran, so every list on screen is painting the zero they no longer carry.
#[tokio::test]
async fn a_sweep_that_rated_something_wakes_the_lists_painting_the_zeros() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = DbPool::test_pool().await?;
    staged_library(&db, &tmp, &[Some(3)]).await?;
    let library_changed = Signal::new();
    let subscriber = library_changed.subscribe();

    import(&db, &library_changed).await?;

    assert!(matches!(subscriber.has_changed(), Ok(true)));
    Ok(())
}
