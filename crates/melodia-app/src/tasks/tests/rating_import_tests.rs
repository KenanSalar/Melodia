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
