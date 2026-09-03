use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use crate::error::AppError;

#[tokio::test]
async fn get_all_artists_excludes_empty() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = queries::artist::get_all_artists(&db).await?;
    // artist_stats filters out artists with track_count = 0,
    // so "Unknown Artist" (sentinel, no tracks) is excluded.
    // Should include "Artist A" + "Artist B" from seeded data.
    assert_eq!(artists.len(), 2);
    assert!(artists.iter().any(|a| a.name == "Artist A"));
    assert!(artists.iter().any(|a| a.name == "Artist B"));
    assert!(!artists.iter().any(|a| a.name == "Unknown Artist"));
    Ok(())
}

#[tokio::test]
async fn get_all_artists_sorted_by_name() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = queries::artist::get_all_artists(&db).await?;
    let names: Vec<&str> = artists.iter().map(|a| a.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    Ok(())
}

#[tokio::test]
async fn get_artist_by_id_happy_path() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = queries::artist::get_all_artists(&db).await?;
    let artist = queries::artist::get_artist_by_id(&db, artists[0].id).await?;
    assert_eq!(artist.name, artists[0].name);
    Ok(())
}

#[tokio::test]
async fn get_artist_by_id_not_found() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let result = queries::artist::get_artist_by_id(&db, 99999).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn get_artists_without_images() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = queries::artist::get_artists_without_images(&db).await?;
    // All test artists have no image — should include at least the ones we inserted
    assert!(artists.len() >= 2);
    Ok(())
}

#[tokio::test]
async fn update_artist_image_path() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = queries::artist::get_all_artists(&db).await?;
    let artist = artists
        .iter()
        .find(|a| a.name == "Artist A")
        .ok_or_else(|| AppError::Validation("Artist A missing".into()))?;

    queries::artist::update_artist_image_path(&db, artist.id, "/images/artist_a.jpg").await?;

    let updated = queries::artist::get_artist_by_id(&db, artist.id).await?;
    assert_eq!(updated.image_path.as_deref(), Some("/images/artist_a.jpg"));

    // Should no longer appear in "without images"
    let without = queries::artist::get_artists_without_images(&db).await?;
    assert!(!without.iter().any(|a| a.id == artist.id));
    Ok(())
}
