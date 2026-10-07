use crate::database::DbPool;
use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use melodia_core::error::AppError;

#[tokio::test]
async fn insert_folder_returns_correct_fields() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let f = queries::folder::insert_folder(&db, "/music", true).await?;
    assert_eq!(f.path, "/music");
    assert!(f.is_enabled);
    assert!(f.id > 0);
    Ok(())
}

#[tokio::test]
async fn get_all_folders_empty_initially() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let folders = queries::folder::get_all_folders(&db).await?;
    assert!(folders.is_empty());
    Ok(())
}

#[tokio::test]
async fn get_all_folders_returns_inserted() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    queries::folder::insert_folder(&db, "/downloads", false).await?;
    let folders = queries::folder::get_all_folders(&db).await?;
    assert_eq!(folders.len(), 2);
    Ok(())
}

#[tokio::test]
async fn get_folder_by_id_happy_path() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let f = queries::folder::insert_folder(&db, "/music", true).await?;
    let found = queries::folder::get_folder_by_id(&db, f.id).await?;
    assert_eq!(found.path, "/music");
    Ok(())
}

#[tokio::test]
async fn get_folder_by_id_not_found() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let result = queries::folder::get_folder_by_id(&db, 99999).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn delete_folder_removes_it() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let f = queries::folder::insert_folder(&db, "/music", true).await?;
    queries::folder::delete_folder(&db, f.id).await?;
    let folders = queries::folder::get_all_folders(&db).await?;
    assert!(folders.is_empty());
    Ok(())
}

#[tokio::test]
async fn delete_folder_cascades_tracks() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let f = queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    // Update folder_id to match our folder
    sqlx::query("UPDATE tracks SET folder_id = ? WHERE file_path = '/music/song.mp3'")
        .bind(f.id)
        .execute(db.write())
        .await?;

    queries::folder::delete_folder(&db, f.id).await?;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
async fn upsert_folder_creates_disabled() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let id = queries::folder::upsert_folder(&mut tx, "/new_folder").await?;
    tx.commit().await?;

    let f = queries::folder::get_folder_by_id(&db, id).await?;
    assert!(!f.is_enabled);
    Ok(())
}

#[tokio::test]
async fn upsert_folder_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let id1 = queries::folder::upsert_folder(&mut tx, "/music").await?;
    let id2 = queries::folder::upsert_folder(&mut tx, "/music").await?;
    tx.commit().await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn update_folder_last_scanned() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let f = queries::folder::insert_folder(&db, "/music", true).await?;
    let ts = "2024-06-15T12:00:00+00:00";
    queries::folder::update_folder_last_scanned(&db, f.id, ts).await?;

    let found = queries::folder::get_folder_by_id(&db, f.id).await?;
    assert_eq!(found.last_scanned.as_deref(), Some(ts));
    Ok(())
}

// === What absorbing or deleting a folder leaves behind ===

/// Files `track_id` under `folder_id`, the fixture filing every track under the first folder.
async fn file_under(db: &DbPool, track_id: i64, folder_id: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE tracks SET folder_id = ? WHERE id = ?")
        .bind(folder_id)
        .bind(track_id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// A parent taking over the folders inside it must not re-import their tracks: the cascade on
/// `tracks.folder_id` would take the rows, and with them what no rescan brings back.
#[tokio::test]
async fn absorbing_a_folder_moves_its_tracks_to_the_parent_with_their_play_state()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let parent = queries::folder::insert_folder(&db, "/music", true).await?;
    let child = queries::folder::insert_folder(&db, "/music/rock", true).await?;
    let id = insert_test_track(&db, "/music/rock/a.mp3", "A", "Artist", "Album", "Rock").await?;
    file_under(&db, id, child.id).await?;
    queries::track::set_rating(&db, &[id], 4).await?;
    queries::track::set_favorite(&db, &[id], true).await?;
    queries::track::add_play_counts(&db, &[(id, 3)], "2026-01-01T00:00:00+00:00").await?;

    queries::folder::absorb_folders(&db, parent.id, &[child.id]).await?;

    let (folder_id, rating, play_count, is_favorite): (i64, i64, i64, bool) = sqlx::query_as(
        "SELECT folder_id, rating, play_count, is_favorite FROM tracks WHERE id = ?",
    )
    .bind(id)
    .fetch_one(db.read())
    .await?;
    assert_eq!((folder_id, rating, play_count, is_favorite), (parent.id, 4, 3, true));
    let folders: Vec<i64> =
        queries::folder::get_all_folders(&db).await?.iter().map(|f| f.id).collect();
    assert_eq!(folders, [parent.id], "the absorbed row goes once its tracks have moved");
    Ok(())
}

/// Nothing else would prune these: only a scan that changed something does, and the album left
/// holding a cover keeps that file referenced, out of the artwork sweep's reach.
#[tokio::test]
async fn deleting_a_folder_prunes_the_album_artist_and_genre_it_emptied() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let removed = queries::folder::insert_folder(&db, "/other", true).await?;
    insert_test_track(&db, "/music/a.mp3", "A", "Kept Artist", "Kept Album", "Rock").await?;
    let id =
        insert_test_track(&db, "/other/b.mp3", "B", "Gone Artist", "Gone Album", "Jazz").await?;
    file_under(&db, id, removed.id).await?;

    queries::folder::delete_folder(&db, removed.id).await?;

    let albums: Vec<String> =
        sqlx::query_scalar("SELECT name FROM albums ORDER BY name").fetch_all(db.read()).await?;
    assert_eq!(albums, ["Kept Album"]);
    let artists: Vec<String> = sqlx::query_scalar("SELECT name FROM artists WHERE id <> ?")
        .bind(queries::artist::UNKNOWN_ARTIST_ID)
        .fetch_all(db.read())
        .await?;
    assert_eq!(artists, ["Kept Artist"]);
    let genres: Vec<String> =
        sqlx::query_scalar("SELECT name FROM genres ORDER BY name").fetch_all(db.read()).await?;
    assert_eq!(genres, ["Rock"]);
    Ok(())
}

/// The playlist showing a cover of the removed folder's tracks would keep that file referenced
/// and on screen with none of its tracks left to hold it.
#[tokio::test]
async fn deleting_a_folder_moves_a_playlist_cover_off_the_tracks_it_took() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let removed = queries::folder::insert_folder(&db, "/other", true).await?;
    let gone = insert_test_track(&db, "/other/a.mp3", "A", "Artist", "Album", "Rock").await?;
    let stays = insert_test_track(&db, "/music/b.mp3", "B", "Artist", "Album", "Rock").await?;
    file_under(&db, gone, removed.id).await?;
    set_test_artwork(&db, gone, "/art/gone.jpg").await?;
    set_test_artwork(&db, stays, "/art/stays.jpg").await?;
    let playlist = queries::playlist::create_playlist(&db, "Mixed", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, playlist.id, &[gone, stays]).await?;

    queries::folder::delete_folder(&db, removed.id).await?;

    let shown = queries::playlist::get_playlist_by_id(&db, playlist.id).await?.thumbnail_path;
    assert_eq!(shown.as_deref(), Some("/art/stays.jpg"));
    Ok(())
}
