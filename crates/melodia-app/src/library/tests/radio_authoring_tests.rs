//! What a re-pasted stream URL does, which is the one branch of adding a station that reaches no
//! socket.
//!
//! Getting it wrong is quiet either way: a second identical card the user cannot tell apart, or a
//! station they had only ever played that never makes it into the kept list however many times
//! they add it.

use melodia_core::entities::radio::NewRadioStation;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

use super::merged_onto_existing;

const STREAM_URL: &str = "https://example.test/stream";

/// A hand-typed row as `add_custom_station` writes one, minus the star.
fn typed() -> NewRadioStation {
    NewRadioStation {
        station_uuid: None,
        name: "Test Station".to_owned(),
        stream_url: STREAM_URL.to_owned(),
        homepage: None,
        favicon_url: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
    }
}

async fn station_count(db: &DbPool) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM radio_stations")
        .fetch_one(db.read())
        .await?)
}

/// A URL nothing holds has to fall through to the probe rather than being answered here, and it
/// must leave the table as it found it — a row written before the connect proves the mount plays
/// is a card for a station that does not work.
#[tokio::test]
async fn a_url_nothing_holds_is_left_to_the_probe() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let merged = merged_onto_existing(&db, STREAM_URL).await?;

    assert_eq!(merged, None);
    assert_eq!(station_count(&db).await?, 0, "and nothing is written on the way past");
    Ok(())
}

/// The merge stars what it finds, so re-adding a station that was only ever played promotes it
/// into the kept list instead of spending a whole connect to produce a second identical card.
#[tokio::test]
async fn a_url_already_here_is_starred_rather_than_added_again() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = queries::radio::save_station(&db, &typed()).await?;

    let merged = merged_onto_existing(&db, STREAM_URL).await?;

    assert_eq!(merged, Some(id), "the row in hand is the answer");
    assert!(queries::radio::get_station_by_id(&db, id).await?.is_favorite);
    assert_eq!(station_count(&db).await?, 1, "and there is still only one of it");
    Ok(())
}
