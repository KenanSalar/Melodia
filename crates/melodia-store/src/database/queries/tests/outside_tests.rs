//! The boot sweep over files opened from outside every library folder: what it deletes, what a
//! playlist or the queue keeps, and that a folder it reads as enabled keeps its rows.

use crate::database::DbPool;
use crate::database::queries::fixtures::{
    execute, fetch_all, fetch_one, insert_test_track, quoted,
};
use crate::database::queries::{self, outside};
use melodia_core::error::AppError;

/// A library folder at `/music`, folder 1, and an outside one at `/downloads`, folder 2.
async fn library() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    queries::folder::insert_folder(&db, "/downloads", false).await?;
    Ok(db)
}

const OUTSIDE: i64 = 2;

/// A track at `path` filed under the outside folder, linked as the seeding helper links every row.
async fn opened(db: &DbPool, path: &str) -> Result<i64, AppError> {
    let id =
        insert_test_track(db, path, path, "Opened Artist", "Opened Album", "Opened Genre").await?;
    execute(db, &format!("UPDATE tracks SET folder_id = {OUTSIDE} WHERE id = {id}")).await?;
    Ok(id)
}

async fn opened_image(db: &DbPool, path: &str) -> Result<[i64; 2], AppError> {
    let first = opened(db, path).await?;
    execute(db, &format!("UPDATE tracks SET cue_track = 1, cue_start = 0 WHERE id = {first}"))
        .await?;
    let second = opened(db, path).await?;
    execute(db, &format!("UPDATE tracks SET cue_track = 2, cue_start = 75 WHERE id = {second}"))
        .await?;
    Ok([first, second])
}

async fn hold_in_playlist(db: &DbPool, ids: &[i64]) -> Result<(), AppError> {
    queries::playlist::create_playlist_with_tracks(db, "Kept".to_owned(), None, ids.to_vec())
        .await?;
    Ok(())
}

async fn track_ids(db: &DbPool) -> Result<Vec<i64>, AppError> {
    fetch_all(db, "SELECT id FROM tracks ORDER BY id").await
}

#[tokio::test]
async fn an_outside_file_nothing_holds_is_deleted() -> Result<(), AppError> {
    let db = library().await?;
    opened(&db, "/downloads/a.flac").await?;

    outside::sweep(&db, &[OUTSIDE], &[]).await?;

    assert_eq!(track_ids(&db).await?, Vec::<i64>::new());
    Ok(())
}

#[tokio::test]
async fn an_outside_file_a_playlist_or_the_queue_holds_is_kept() -> Result<(), AppError> {
    let db = library().await?;
    let in_playlist = opened(&db, "/downloads/playlist.flac").await?;
    let queued = opened(&db, "/downloads/queued.flac").await?;
    opened(&db, "/downloads/neither.flac").await?;
    hold_in_playlist(&db, &[in_playlist]).await?;

    outside::sweep(&db, &[OUTSIDE], &[queued]).await?;

    assert_eq!(track_ids(&db).await?, [in_playlist, queued]);
    Ok(())
}

#[tokio::test]
async fn one_track_held_keeps_its_whole_image() -> Result<(), AppError> {
    let db = library().await?;
    let [first, second] = opened_image(&db, "/downloads/held.flac").await?;
    opened_image(&db, "/downloads/unheld.flac").await?;
    hold_in_playlist(&db, &[second]).await?;

    outside::sweep(&db, &[OUTSIDE], &[]).await?;

    assert_eq!(track_ids(&db).await?, [first, second]);
    Ok(())
}

#[tokio::test]
async fn a_library_folder_s_rows_are_never_swept() -> Result<(), AppError> {
    let db = library().await?;
    let library_track =
        insert_test_track(&db, "/music/a.flac", "A", "Artist", "Album", "Rock").await?;

    outside::sweep(&db, &[1, OUTSIDE], &[]).await?;

    assert_eq!(track_ids(&db).await?, [library_track]);
    Ok(())
}

#[tokio::test]
async fn a_folder_enabled_since_it_was_read_is_left_alone() -> Result<(), AppError> {
    let db = library().await?;
    let track = opened(&db, "/downloads/a.flac").await?;
    execute(&db, &format!("UPDATE folders SET is_enabled = TRUE WHERE id = {OUTSIDE}")).await?;

    outside::sweep(&db, &[OUTSIDE], &[]).await?;

    assert_eq!(track_ids(&db).await?, [track]);
    Ok(())
}

#[tokio::test]
async fn a_kept_outside_row_still_linked_is_unlinked() -> Result<(), AppError> {
    let db = library().await?;
    let track = opened(&db, "/downloads/a.flac").await?;
    hold_in_playlist(&db, &[track]).await?;

    outside::sweep(&db, &[OUTSIDE], &[]).await?;

    let links: (Option<i64>, Option<i64>, Option<i64>, i64) = fetch_one(
        &db,
        &format!(
            "SELECT album_id, artist_id, genre_id, \
             (SELECT COUNT(*) FROM track_artists WHERE track_id = {track}) \
             + (SELECT COUNT(*) FROM track_genres WHERE track_id = {track}) \
             FROM tracks WHERE id = {track}"
        ),
    )
    .await?;
    assert_eq!(links, (None, None, None, 0));
    Ok(())
}

#[tokio::test]
async fn the_albums_artists_and_genres_only_outside_rows_held_are_pruned() -> Result<(), AppError> {
    let db = library().await?;
    let track = opened(&db, "/downloads/a.flac").await?;
    hold_in_playlist(&db, &[track]).await?;

    outside::sweep(&db, &[OUTSIDE], &[]).await?;

    let names = quoted("Opened Album");
    let albums: i64 =
        fetch_one(&db, &format!("SELECT COUNT(*) FROM albums WHERE name = {names}")).await?;
    assert_eq!(albums, 0);
    Ok(())
}

#[tokio::test]
async fn an_outside_folder_left_empty_goes_and_one_still_holding_a_row_stays()
-> Result<(), AppError> {
    let db = library().await?;
    queries::folder::insert_folder(&db, "/elsewhere", false).await?;
    let kept = opened(&db, "/downloads/kept.flac").await?;
    let swept = insert_test_track(&db, "/elsewhere/b.flac", "B", "Artist", "Album", "Rock").await?;
    execute(&db, &format!("UPDATE tracks SET folder_id = 3 WHERE id = {swept}")).await?;
    hold_in_playlist(&db, &[kept]).await?;

    outside::sweep(&db, &[OUTSIDE, 3], &[]).await?;

    let folders: Vec<String> = fetch_all(&db, "SELECT path FROM folders ORDER BY id").await?;
    assert_eq!(folders, ["/music", "/downloads"]);
    Ok(())
}
