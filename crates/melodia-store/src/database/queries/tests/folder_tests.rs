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
