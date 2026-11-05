//! The duplicate finder's Remove: what a keeper takes from its spares, and what is left of them.
//!
//! The history columns are the one record of a user's listening that no rescan rebuilds, so each
//! merge rule is pinned on its own.

use super::*;
use crate::database::queries::fixtures::{insert_test_track, make_test_metadata, numbered_library};
use crate::database::rows::FromRow;
use crate::database::sql::{self, Sql};

/// Spares past [`BULK_THRESHOLD`], so the merge runs with the stats triggers dropped.
const BULK_SPARES: usize = BULK_THRESHOLD + 1;

/// An `excluded_files` row: path, hash, size, mtime and folder.
type Exclusion = (String, Option<String>, Option<i64>, Option<String>, i64);

/// A library of three distinct files, the first kept and the other two its spares.
async fn keeper_and_two_spares() -> Result<(DbPool, i64, [i64; 2]), AppError> {
    let (db, ids) = numbered_library(3).await?;
    Ok((db, ids[0], [ids[1], ids[2]]))
}

async fn set(db: &DbPool, id: i64, assignments: &str) -> Result<(), AppError> {
    let mut update = Sql::trusted(format!("UPDATE tracks SET {assignments} WHERE id = ?"));
    update.bind(id);
    db.write(move |conn| update.execute(conn)).await?;
    Ok(())
}

async fn column<T>(db: &DbPool, id: i64, name: &str) -> Result<T, AppError>
where
    T: FromRow + Send + 'static,
{
    let mut select = Sql::trusted(format!("SELECT {name} FROM tracks WHERE id = ?"));
    select.bind(id);
    db.read(move |conn| select.fetch_one(conn)).await
}

async fn remove(db: &DbPool, keeper: i64, spares: &[i64]) -> Result<RemovedDuplicates, AppError> {
    remove_duplicates(db, vec![GroupRemoval { keeper, spares: spares.to_vec() }]).await
}

async fn playlist_order(db: &DbPool, playlist_id: i64) -> Result<Vec<i64>, AppError> {
    let tracks = crate::database::queries::playlist::get_playlist_tracks(db, playlist_id).await?;
    Ok(tracks.into_iter().map(|track| track.id).collect())
}

/// A row gone since the finder read it: a watcher batch or a scan took it.
async fn delete(db: &DbPool, id: i64) -> Result<(), AppError> {
    db.write(move |conn| sql::execute(conn, "DELETE FROM tracks WHERE id = ?", (id,))).await?;
    Ok(())
}

async fn track_count(db: &DbPool) -> Result<i64, AppError> {
    db.read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM tracks", ())).await
}

#[tokio::test]
async fn a_removal_sums_its_spares_plays_and_skips_onto_the_keeper() -> Result<(), AppError> {
    let (db, keeper, [first, second]) = keeper_and_two_spares().await?;
    set(&db, keeper, "play_count = 2, skip_count = 1").await?;
    set(&db, first, "play_count = 3, skip_count = 0").await?;
    set(&db, second, "play_count = 4, skip_count = 2").await?;

    remove(&db, keeper, &[first, second]).await?;

    let counts =
        (column::<i64>(&db, keeper, "play_count").await?, column(&db, keeper, "skip_count").await?);
    assert_eq!(counts, (9, 3));
    Ok(())
}

#[tokio::test]
async fn the_keeper_takes_the_latest_play_of_its_group() -> Result<(), AppError> {
    let (db, keeper, [first, second]) = keeper_and_two_spares().await?;
    set(&db, first, "last_played = '2024-05-01T00:00:00+00:00'").await?;
    set(&db, second, "last_played = '2023-05-01T00:00:00+00:00'").await?;

    remove(&db, keeper, &[first, second]).await?;

    let last_played: Option<String> = column(&db, keeper, "last_played").await?;
    assert_eq!(last_played.as_deref(), Some("2024-05-01T00:00:00+00:00"));
    Ok(())
}

#[tokio::test]
async fn the_keeper_takes_the_earliest_added_date_of_its_group() -> Result<(), AppError> {
    let (db, keeper, [first, second]) = keeper_and_two_spares().await?;
    set(&db, keeper, "date_added = '2024-06-01T00:00:00+00:00'").await?;
    set(&db, first, "date_added = '2022-01-01T00:00:00+00:00'").await?;
    set(&db, second, "date_added = '2021-03-01T00:00:00+00:00'").await?;

    remove(&db, keeper, &[first, second]).await?;

    let added: String = column(&db, keeper, "date_added").await?;
    assert_eq!(added, "2021-03-01T00:00:00+00:00");
    Ok(())
}

/// Reported for `favorites::merged_onto`, which carries the heart on to a love.
#[tokio::test]
async fn a_heart_on_a_spare_lands_on_the_keeper_and_is_reported() -> Result<(), AppError> {
    let (db, keeper, [first, second]) = keeper_and_two_spares().await?;
    set(&db, second, "is_favorite = 1").await?;

    let removed = remove(&db, keeper, &[first, second]).await?;

    assert!(column::<bool>(&db, keeper, "is_favorite").await?, "the keeper holds the heart");
    assert_eq!(removed.hearted, [keeper], "and the app is told to sync it");
    Ok(())
}

#[tokio::test]
async fn a_keeper_without_a_rating_takes_its_spares_highest() -> Result<(), AppError> {
    let (db, keeper, [first, second]) = keeper_and_two_spares().await?;
    set(&db, first, "rating = 3").await?;
    set(&db, second, "rating = 5").await?;

    let removed = remove(&db, keeper, &[first, second]).await?;

    assert_eq!(column::<i32>(&db, keeper, "rating").await?, 5);
    assert_eq!(removed.rated, [(keeper, 5)], "the app writes the rating back to the keeper's file");
    Ok(())
}

#[tokio::test]
async fn a_keepers_own_rating_outranks_its_spares() -> Result<(), AppError> {
    let (db, keeper, [first, second]) = keeper_and_two_spares().await?;
    set(&db, keeper, "rating = 2").await?;
    set(&db, first, "rating = 5").await?;

    let removed = remove(&db, keeper, &[first, second]).await?;

    assert_eq!(column::<i32>(&db, keeper, "rating").await?, 2);
    assert!(removed.rated.is_empty(), "nothing changed that the file owes");
    Ok(())
}

#[tokio::test]
async fn a_spares_playlist_place_passes_to_the_keeper_at_its_position() -> Result<(), AppError> {
    let (db, ids) = numbered_library(4).await?;
    let (keeper, spare, before, after) = (ids[0], ids[1], ids[2], ids[3]);
    let playlist = crate::database::queries::playlist::create_playlist_with_tracks(
        &db,
        "Mix".to_owned(),
        None,
        vec![before, spare, after],
    )
    .await?;

    remove(&db, keeper, &[spare]).await?;

    assert_eq!(playlist_order(&db, playlist).await?, [before, keeper, after]);
    Ok(())
}

/// The spare's place goes rather than repeating the keeper, and the positions close over its gap.
#[tokio::test]
async fn a_spare_in_a_playlist_the_keeper_is_already_in_leaves_only_the_keepers_place()
-> Result<(), AppError> {
    let (db, ids) = numbered_library(3).await?;
    let (keeper, spare, other) = (ids[0], ids[1], ids[2]);
    let playlist = crate::database::queries::playlist::create_playlist_with_tracks(
        &db,
        "Mix".to_owned(),
        None,
        vec![keeper, spare, other],
    )
    .await?;

    remove(&db, keeper, &[spare]).await?;

    let places: Vec<(i64, i64)> = db
        .read(move |conn| {
            sql::fetch_all(
                conn,
                "SELECT track_id, position FROM playlist_items WHERE playlist_id = ? \
                 ORDER BY position",
                (playlist,),
            )
        })
        .await?;
    assert_eq!(places, [(keeper, 0), (other, 1)]);
    Ok(())
}

/// The spare placed first has the higher id, so its place isn't the first one the read returns.
#[tokio::test]
async fn a_keeper_with_two_spares_in_one_playlist_takes_the_earlier_place() -> Result<(), AppError>
{
    let (db, ids) = numbered_library(4).await?;
    let (keeper, lower_spare, higher_spare, other) = (ids[0], ids[1], ids[2], ids[3]);
    let playlist = crate::database::queries::playlist::create_playlist_with_tracks(
        &db,
        "Mix".to_owned(),
        None,
        vec![higher_spare, other, lower_spare],
    )
    .await?;

    remove(&db, keeper, &[lower_spare, higher_spare]).await?;

    assert_eq!(playlist_order(&db, playlist).await?, [keeper, other]);
    Ok(())
}

/// The earlier place holds the later date, so the date is that spare's, not the group's earliest.
#[tokio::test]
async fn a_keeper_takes_the_added_date_of_the_earlier_place() -> Result<(), AppError> {
    let (db, ids) = numbered_library(3).await?;
    let (keeper, lower_spare, higher_spare) = (ids[0], ids[1], ids[2]);
    let playlist = crate::database::queries::playlist::create_playlist_with_tracks(
        &db,
        "Mix".to_owned(),
        None,
        vec![higher_spare, lower_spare],
    )
    .await?;
    set_added_at(&db, playlist, higher_spare, "2024-02-01T00:00:00+00:00").await?;
    set_added_at(&db, playlist, lower_spare, "2022-02-01T00:00:00+00:00").await?;

    remove(&db, keeper, &[lower_spare, higher_spare]).await?;

    let added: String = db
        .read(move |conn| {
            sql::fetch_one(
                conn,
                "SELECT added_at FROM playlist_items WHERE playlist_id = ? AND track_id = ?",
                (playlist, keeper),
            )
        })
        .await?;
    assert_eq!(added, "2024-02-01T00:00:00+00:00");
    Ok(())
}

async fn set_added_at(
    db: &DbPool,
    playlist_id: i64,
    track_id: i64,
    added_at: &'static str,
) -> Result<(), AppError> {
    db.write(move |conn| {
        sql::execute(
            conn,
            "UPDATE playlist_items SET added_at = ? WHERE playlist_id = ? AND track_id = ?",
            (added_at, playlist_id, track_id),
        )
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_removed_spare_leaves_the_library() -> Result<(), AppError> {
    let (db, keeper, spares) = keeper_and_two_spares().await?;

    let removed = remove(&db, keeper, &spares).await?;

    assert_eq!(removed.removed, 2);
    assert_eq!(track_count(&db).await?, 1, "only the keeper is left");
    Ok(())
}

/// The stamp is what a scan compares, so a spare unchanged on disk is passed by unread.
#[tokio::test]
async fn a_removed_spares_file_is_kept_out_with_its_hash_and_stamp() -> Result<(), AppError> {
    let (db, keeper, [first, _]) = keeper_and_two_spares().await?;

    remove(&db, keeper, &[first]).await?;

    let kept_out: Vec<Exclusion> = db
        .read(|conn| {
            sql::fetch_all(
                conn,
                "SELECT path, file_hash, file_size, date_modified, folder_id FROM excluded_files",
                (),
            )
        })
        .await?;
    let spare = make_test_metadata("Track 0001");
    assert_eq!(
        kept_out,
        [(
            "/music/0001.mp3".to_owned(),
            Some(spare.file_hash),
            Some(spare.file_size),
            spare.date_modified,
            1,
        )]
    );
    Ok(())
}

#[tokio::test]
async fn a_spare_without_a_hash_is_kept_out_unstamped() -> Result<(), AppError> {
    let (db, keeper, [first, _]) = keeper_and_two_spares().await?;
    set(&db, first, "file_hash = NULL").await?;

    remove(&db, keeper, &[first]).await?;

    let stamp: Vec<(Option<i64>, Option<String>)> = db
        .read(|conn| {
            sql::fetch_all(conn, "SELECT file_size, date_modified FROM excluded_files", ())
        })
        .await?;
    assert_eq!(stamp, [(None, None)]);
    Ok(())
}

#[tokio::test]
async fn a_removal_whose_keeper_is_gone_removes_nothing_and_skips_its_spares()
-> Result<(), AppError> {
    let (db, keeper, spares) = keeper_and_two_spares().await?;
    delete(&db, keeper).await?;

    let removed = remove(&db, keeper, &spares).await?;

    assert_eq!((removed.removed, removed.skipped), (0, 2));
    assert_eq!(track_count(&db).await?, 2, "both spares are still in the library");
    Ok(())
}

#[tokio::test]
async fn a_spare_already_gone_is_counted_as_skipped() -> Result<(), AppError> {
    let (db, keeper, [first, second]) = keeper_and_two_spares().await?;
    delete(&db, second).await?;

    let removed = remove(&db, keeper, &[first, second]).await?;

    assert_eq!((removed.removed, removed.skipped), (1, 1));
    Ok(())
}

/// The bulk path drops the stats triggers for its deletes, and every later writer relies on them
/// being back.
#[tokio::test]
async fn a_bulk_removal_leaves_the_stats_triggers_live() -> Result<(), AppError> {
    let (db, ids) = numbered_library(BULK_SPARES + 1).await?;
    remove(&db, ids[0], &ids[1..]).await?;
    let after_removal = artist_track_count(&db).await?;

    insert_test_track(&db, "/music/added.mp3", "Added", "Artist", "Album", "Rock").await?;

    assert_eq!(artist_track_count(&db).await?, after_removal + 1);
    Ok(())
}

/// With the triggers dropped nothing kept the counts while the spares went, so the recount after
/// is the only thing that brings them back.
#[tokio::test]
async fn a_bulk_removal_leaves_the_counts_a_recount_gives() -> Result<(), AppError> {
    let (db, ids) = numbered_library(BULK_SPARES + 1).await?;

    remove(&db, ids[0], &ids[1..]).await?;

    assert_eq!(artist_track_count(&db).await?, 1);
    Ok(())
}

async fn artist_track_count(db: &DbPool) -> Result<i64, AppError> {
    db.read(|conn| {
        sql::fetch_one(conn, "SELECT track_count FROM artists WHERE name = 'Artist'", ())
    })
    .await
}
