#[allow(clippy::wildcard_imports)]
use crate::database::queries::tests::helpers::*;
use crate::database::queries;
use crate::error::AppError;

#[tokio::test]
async fn get_all_albums_from_seeded_db() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let albums = queries::album::get_all_albums(&db).await?;
    assert_eq!(albums.len(), 2); // "Album One" and "Album Two"
    // Sorted by name — "Album One" before "Album Two"
    assert_eq!(albums[0].name, "Album One");
    assert_eq!(albums[1].name, "Album Two");
    Ok(())
}

#[tokio::test]
async fn get_album_by_id_happy_path() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let albums = queries::album::get_all_albums(&db).await?;
    let found = queries::album::get_album_by_id(&db, albums[0].id).await?;
    assert_eq!(found.name, albums[0].name);
    Ok(())
}

#[tokio::test]
async fn get_album_by_id_not_found() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let result = queries::album::get_album_by_id(&db, 99999).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn get_albums_by_artist() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artist_id: i64 = sqlx::query_scalar("SELECT id FROM artists WHERE name = 'Artist A'")
        .fetch_one(db.read())
        .await?;
    let albums = queries::album::get_albums_by_artist(&db, artist_id).await?;
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].name, "Album One");
    Ok(())
}

#[tokio::test]
async fn get_all_albums_track_counts() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let albums = queries::album::get_all_albums(&db).await?;
    // "Album One" has 2 tracks (track1 + track3), "Album Two" has 1
    let one = albums
        .iter()
        .find(|a| a.name == "Album One")
        .ok_or_else(|| AppError::Validation("Album One missing".into()))?;
    let two = albums
        .iter()
        .find(|a| a.name == "Album Two")
        .ok_or_else(|| AppError::Validation("Album Two missing".into()))?;
    assert_eq!(one.track_count, 2);
    assert_eq!(two.track_count, 1);
    Ok(())
}
