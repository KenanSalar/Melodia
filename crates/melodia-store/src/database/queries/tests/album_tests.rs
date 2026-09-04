use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use melodia_core::error::AppError;

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

#[tokio::test]
async fn set_album_artwork_replaces_existing_cover() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let album_id: i64 = sqlx::query_scalar("SELECT id FROM albums WHERE name = 'Album One'")
        .fetch_one(db.read())
        .await?;

    // Give the album an existing cover — the roll-up refuses to touch this case.
    sqlx::query("UPDATE albums SET artwork_path = ? WHERE id = ?")
        .bind("/covers/old.jpg")
        .bind(album_id)
        .execute(db.write())
        .await?;

    let mut tx = db.write().begin().await?;
    queries::album::set_album_artwork(&mut tx, &[album_id], Some("/covers/new.jpg")).await?;
    tx.commit().await?;

    let art: Option<String> = sqlx::query_scalar("SELECT artwork_path FROM albums WHERE id = ?")
        .bind(album_id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(art.as_deref(), Some("/covers/new.jpg"));
    Ok(())
}

#[tokio::test]
async fn prune_orphans_removes_emptied_album_artist_and_genre() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    // "Album Two" / "Artist B" / genre "Pop" each have a single track (track2);
    // deleting it strands all three, while genre "Rock" (track1/track3) survives.
    let mut tx = db.write().begin().await?;
    let deleted = queries::scan::delete_track_by_path(&mut tx, "/music/track2.mp3").await?;
    assert!(deleted);
    queries::scan::prune_orphans(&mut tx).await?;
    tx.commit().await?;

    let albums: Vec<String> =
        sqlx::query_scalar("SELECT name FROM albums ORDER BY name").fetch_all(db.read()).await?;
    assert_eq!(albums, vec!["Album One".to_owned()]);

    let artist_b: Option<i64> =
        sqlx::query_scalar("SELECT id FROM artists WHERE name = 'Artist B'")
            .fetch_optional(db.read())
            .await?;
    assert!(artist_b.is_none(), "orphaned Artist B should be pruned");

    // A still-used artist and the id-1 "unknown" default both survive.
    let artist_a: Option<i64> =
        sqlx::query_scalar("SELECT id FROM artists WHERE name = 'Artist A'")
            .fetch_optional(db.read())
            .await?;
    assert!(artist_a.is_some());
    let unknown: Option<i64> =
        sqlx::query_scalar("SELECT id FROM artists WHERE id = 1").fetch_optional(db.read()).await?;
    assert!(unknown.is_some(), "the id-1 unknown default must never be pruned");

    // The emptied genre is pruned; a still-used genre survives.
    let genres: Vec<String> =
        sqlx::query_scalar("SELECT name FROM genres ORDER BY name").fetch_all(db.read()).await?;
    assert_eq!(genres, vec!["Rock".to_owned()]);

    Ok(())
}
