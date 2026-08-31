//! The reference set the artwork sweep deletes against.
//!
//! A missing column here is not a compile error and not a wrong-looking result — it is a sweep
//! that unlinks live artwork, so both halves are pinned: that the SQL names every column, and that
//! the ones reachable through no other column come back.

use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::{insert_test_track, setup_seeded_db};
use crate::entities::radio;
use crate::error::AppError;

use super::{ARTWORK_COLUMNS, REFERENCED_PATHS, repoint_all};

fn test_station() -> radio::NewRadioStation {
    radio::NewRadioStation {
        name: "Test Station".to_owned(),
        stream_url: "http://example.invalid/stream".to_owned(),
        ..Default::default()
    }
}

/// Every column in the schema that stores a path into the artwork directories, restated
/// independently of the production ledger so that dropping one from either side fails.
///
/// Spelled as `(table, column)` so the walk can't be satisfied by a column name that happens to
/// appear in another arm's text — `artwork_path` is on two tables.
const EXPECTED_COLUMNS: [(&str, &str); 6] = [
    ("tracks", "artwork_path"),
    ("albums", "artwork_path"),
    ("artists", "image_path"),
    ("playlists", "thumbnail_path"),
    ("radio_stations", "artwork_path"),
    ("radio_logo_answers", "artwork_path"),
];

/// The sweep keeps only what the reference query returns and the renormalize pass re-points only
/// what the ledger names, so a column either forgets is artwork deleted while a row still points at
/// it. `playlists.thumbnail_path` is the one that was missing from the first draft, and the one a
/// reviewer is least likely to miss twice.
#[test]
fn the_reference_query_names_every_artwork_column() {
    assert_eq!(ARTWORK_COLUMNS, EXPECTED_COLUMNS, "the artwork column ledger has moved");

    for (table, column) in EXPECTED_COLUMNS {
        assert!(
            REFERENCED_PATHS.contains(&format!("{column} FROM {table}")),
            "the reference query no longer selects {table}.{column}, so a sweep will delete \
             artwork that column still points at"
        );
    }
    assert_eq!(
        REFERENCED_PATHS.matches("SELECT").count(),
        EXPECTED_COLUMNS.len(),
        "the query has an arm `EXPECTED_COLUMNS` doesn't name, or names one twice"
    );
}

/// The write side has the same shape of failure as the read side: a column the renormalize pass
/// forgets to re-point keeps naming a file that pass has just orphaned, and the next sweep deletes
/// it out from under the row. Exercised rather than grepped — the statements are generated, so a
/// string check would only be reading the ledger back to itself.
#[tokio::test]
async fn repointing_moves_every_artwork_column_at_once() -> Result<(), AppError> {
    const OLD: &str = "/data/artwork/33fb807d1f1b7cbb.png";
    const NEW: &str = "/data/artwork/4cccaf4d4b4cea11.jpg";

    let db = setup_seeded_db().await?;
    // The seed covers tracks, albums and artists only, and the assertions below need a row in
    // every table the ledger names or `moved > 0` can't hold for the empty ones.
    queries::playlist::create_playlist(&db, "Mosaic", None).await?;
    queries::radio::save_station(&db, &test_station()).await?;
    queries::radio::record_logo_hit(
        &db,
        "https://example.invalid/logo.png",
        OLD,
        1_024,
        "2026-08-24T00:00:00.000+00:00",
    )
    .await?;

    let mut tx = db.write().begin().await?;
    for (table, column) in EXPECTED_COLUMNS {
        let sql = format!("UPDATE {table} SET {column} = ?");
        sqlx::query(AssertSqlSafe(sql)).bind(OLD).execute(&mut *tx).await?;
    }
    tx.commit().await?;

    assert!(repoint_all(&db, &[(OLD.to_owned(), NEW.to_owned())]).await? > 0);

    for (table, column) in EXPECTED_COLUMNS {
        let stale: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE {column} = ?"
        )))
        .bind(OLD)
        .fetch_one(db.read())
        .await?;
        let moved: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE {column} = ?"
        )))
        .bind(NEW)
        .fetch_one(db.read())
        .await?;
        assert_eq!(stale, 0, "nothing re-points {table}.{column}, so a renormalized cover dangles");
        assert!(moved > 0, "{table}.{column} lost its path instead of moving it");
    }
    Ok(())
}

/// A custom playlist mosaic is written by `compose_artwork` and pointed at by nothing else, so it
/// is the case a three-column union silently blanks.
#[tokio::test]
async fn a_composite_referenced_only_by_a_playlist_is_still_referenced() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let composite = "/data/artwork/33fb807d1f1b7cbb.jpg";

    let playlist = queries::playlist::create_playlist(&db, "Mosaic", None).await?;
    queries::playlist::set_playlist_custom_thumbnail(&db, playlist.id, composite).await?;

    let referenced = queries::artwork::referenced_filenames(&db).await?;

    assert!(
        referenced.contains("33fb807d1f1b7cbb.jpg"),
        "a composite reachable only through `playlists.thumbnail_path` must survive the sweep; \
         got {referenced:?}"
    );
    Ok(())
}

/// The station sibling of the case above, and the sharper one: a logo came off a third-party host
/// that is often dead by the time the sweep runs, so unlike every other arm of the union there is
/// no re-scan that can put it back.
#[tokio::test]
async fn a_logo_referenced_only_by_a_station_is_still_referenced() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let logo = "/data/artwork/33fb807d1f1b7cbb.jpg";

    let station_id = queries::radio::save_station(&db, &test_station()).await?;
    queries::radio::set_artwork(&db, station_id, Some(logo)).await?;

    let referenced = queries::artwork::referenced_filenames(&db).await?;

    assert!(
        referenced.contains("33fb807d1f1b7cbb.jpg"),
        "a logo reachable only through `radio_stations.artwork_path` must survive the sweep; \
         got {referenced:?}"
    );
    Ok(())
}

/// The sweep compares against a directory listing, so a stored absolute path has to reduce to the
/// name on disk — including for a row written before the data directory moved.
#[tokio::test]
async fn paths_are_reduced_to_the_name_on_disk() -> Result<(), AppError> {
    // Seeded rather than bare: `tracks.folder_id` is a foreign key, and the seeded tracks carry
    // no artwork so they contribute nothing to the set under test.
    let db = setup_seeded_db().await?;
    let track_id = insert_test_track(&db, "/music/a.mp3", "A", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    let stored = "/some/other/root/artwork/abcdef0123456789.png";
    queries::track::set_track_artwork(&mut tx, &[track_id], Some(stored)).await?;
    tx.commit().await?;

    let referenced = queries::artwork::referenced_filenames(&db).await?;

    assert!(
        referenced.contains("abcdef0123456789.png"),
        "expected the basename, got {referenced:?}"
    );
    Ok(())
}

/// An empty string is as good as NULL on these columns and must not reach the set — it reduces to
/// no basename at all, and a `""` entry would match nothing while looking like it had.
#[tokio::test]
async fn blank_and_missing_paths_contribute_nothing() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_id = insert_test_track(&db, "/music/b.mp3", "B", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    queries::track::set_track_artwork(&mut tx, &[track_id], Some("")).await?;
    tx.commit().await?;

    assert!(queries::artwork::referenced_filenames(&db).await?.is_empty());
    Ok(())
}
