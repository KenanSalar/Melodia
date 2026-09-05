//! What keeping a browsed station writes, and what a play reports about it.
//!
//! Both halves are silent when they go wrong. A logo that stops being re-pointed leaves the card
//! showing the previous brand's icon, which reads as the directory being out of date; a click
//! reported for a station the directory does not know is a request nobody asked for and nothing
//! reports either.

use melodia_core::entities::radio::DirectoryStation;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

use super::{click_uuid, keep_directory_station, keep_station};

/// One browse result. Spelled out rather than defaulted: `DirectoryStation` has no `Default`, a
/// station with no uuid and no URL being one nothing may keep.
fn browsed(station_uuid: &str) -> DirectoryStation {
    DirectoryStation {
        station_uuid: station_uuid.to_owned(),
        name: "Test Station".to_owned(),
        stream_url: "https://example.test/stream".to_owned(),
        homepage: None,
        favicon_url: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        state: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        votes: 0,
        click_count: 0,
        last_check_ok: true,
    }
}

async fn artwork_of(db: &DbPool, id: i64) -> Result<Option<String>, AppError> {
    Ok(queries::radio::get_station_by_id(db, id).await?.artwork_path)
}

/// `save_station`'s conflict clause deliberately yields on `artwork_path`, so the `logo.is_some()`
/// guard is the whole of what stops a re-browse blanking a logo an earlier session fetched.
#[tokio::test]
async fn a_re_import_with_no_logo_in_hand_leaves_the_stored_one_alone() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let station = browsed("uuid-1");

    let id = keep_station(&db, &station, Some("artwork/first.png")).await?;
    let again = keep_station(&db, &station, None).await?;

    assert_eq!(again, id, "the uuid is UNIQUE, so a re-browse lands on the same row");
    assert_eq!(artwork_of(&db, id).await?.as_deref(), Some("artwork/first.png"));
    Ok(())
}

/// The other side of the same guard. The caller fetched from the `favicon_url` it had in hand, so
/// a station whose logo moved has to take what that returned — the conflict clause will not, and
/// nothing else in the tree re-points a row that already has a path.
#[tokio::test]
async fn a_re_import_carrying_a_logo_re_points_the_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let station = browsed("uuid-1");

    let id = keep_station(&db, &station, Some("artwork/first.png")).await?;
    keep_station(&db, &station, Some("artwork/moved.png")).await?;

    assert_eq!(artwork_of(&db, id).await?.as_deref(), Some("artwork/moved.png"));
    Ok(())
}

/// The crossing writes the row whichever way the star went, which is what makes deleting an
/// unstarred one the calling surface's job rather than this door's — the argument
/// `delete_if_unlisted` exists to finish.
#[tokio::test]
async fn releasing_a_browsed_station_still_leaves_its_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let id = keep_directory_station(&db, &browsed("uuid-1"), false, None).await?;

    let station = queries::radio::get_station_by_id(&db, id).await?;
    assert!(!station.is_favorite, "the star is what the caller asked to clear");
    assert_eq!(station.station_uuid.as_deref(), Some("uuid-1"), "and the row it hangs on stays");
    Ok(())
}

/// Every reason a play reports nothing. The setting is the user's, and the two empty handles are
/// what a hand-typed or imported row carries — neither names anything the directory could count.
#[test]
fn a_play_reports_only_a_station_the_directory_named() {
    assert_eq!(click_uuid(true, Some("uuid-1")), Some("uuid-1"));
    assert_eq!(click_uuid(false, Some("uuid-1")), None, "the switch is the user's");
    assert_eq!(click_uuid(true, None), None, "a hand-typed station has no uuid");
    assert_eq!(click_uuid(true, Some("")), None, "and an imported row can carry a blank one");
}
