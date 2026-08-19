use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::tests::helpers::*;
use crate::error::AppError;

#[tokio::test]
async fn get_all_genres_from_seeded_db() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let genres = queries::genre::get_all_genres(&db).await?;
    assert_eq!(genres.len(), 2); // "Rock" and "Pop"
    Ok(())
}

#[tokio::test]
async fn get_all_genres_sorted_by_name() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let genres = queries::genre::get_all_genres(&db).await?;
    assert_eq!(genres[0].name, "Pop");
    assert_eq!(genres[1].name, "Rock");
    Ok(())
}

#[tokio::test]
async fn get_genre_by_id_happy_path() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let genres = queries::genre::get_all_genres(&db).await?;
    let genre = queries::genre::get_genre_by_id(&db, genres[0].id).await?;
    assert_eq!(genre.name, genres[0].name);
    Ok(())
}

#[tokio::test]
async fn get_genre_by_id_not_found() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let result = queries::genre::get_genre_by_id(&db, 99999).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn genre_stats_track_count() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let genres = queries::genre::get_all_genres(&db).await?;
    let rock = genres
        .iter()
        .find(|g| g.name == "Rock")
        .ok_or_else(|| AppError::Validation("Rock genre missing".into()))?;
    let pop = genres
        .iter()
        .find(|g| g.name == "Pop")
        .ok_or_else(|| AppError::Validation("Pop genre missing".into()))?;
    // Seeded: track1 (Rock), track2 (Pop), track3 (Rock)
    assert_eq!(rock.track_count, 2);
    assert_eq!(pop.track_count, 1);
    Ok(())
}
