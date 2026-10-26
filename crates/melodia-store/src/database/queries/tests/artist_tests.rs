use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use melodia_core::entities::artist::FavoriteArtist;
use melodia_core::error::AppError;

#[tokio::test]
async fn the_artists_grid_leaves_out_an_artist_with_no_tracks() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = listed_artists(&db).await?;
    // "Unknown Artist" is the sentinel and holds no tracks.
    assert_eq!(artists.len(), 2);
    assert!(artists.iter().any(|a| a.name == "Artist A"));
    assert!(artists.iter().any(|a| a.name == "Artist B"));
    assert!(!artists.iter().any(|a| a.name == "Unknown Artist"));
    Ok(())
}

#[tokio::test]
async fn the_artists_grid_lists_by_name() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = listed_artists(&db).await?;
    let names: Vec<&str> = artists.iter().map(|a| a.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    Ok(())
}

#[tokio::test]
async fn get_artist_by_id_happy_path() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = listed_artists(&db).await?;
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
    // A cutoff every recorded miss is older than; the seed records none.
    let artists = queries::artist::get_artists_without_images(&db, i64::MAX).await?;
    // The seed's two, and not the schema's Unknown Artist sentinel, which has no image either:
    // a directory search for its name answers with a stranger's photo.
    assert_eq!(artists.len(), 2);
    assert!(
        artists.iter().all(|artist| artist.id != queries::artist::UNKNOWN_ARTIST_ID),
        "the placeholder must never be looked up"
    );
    Ok(())
}

/// The cutoff the miss tests ask at, standing for now less the retry window.
const CUTOFF: i64 = 1_000_000;

/// A search that found nothing is retried once it is older than the window, and a miss on the
/// cutoff still counts as recent. Written raw, so the recorder isn't a second reason to fail.
#[tokio::test]
async fn an_artist_sits_the_search_out_until_its_miss_is_older_than_the_cutoff()
-> Result<(), AppError> {
    let rows =
        [(None, true), (Some(CUTOFF - 1), true), (Some(CUTOFF), false), (Some(CUTOFF + 1), false)];
    for (missed_at, listed) in rows {
        let db = setup_seeded_db().await?;
        sqlx::query("UPDATE artists SET image_search_missed_at = ? WHERE name = 'Artist A'")
            .bind(missed_at)
            .execute(db.write())
            .await?;

        let artists = queries::artist::get_artists_without_images(&db, CUTOFF).await?;

        let artist_a_listed = artists.iter().any(|artist| artist.name == "Artist A");
        assert_eq!(artist_a_listed, listed, "a miss recorded at {missed_at:?}");
    }
    Ok(())
}

/// Stamped on the artists named, at the time given, and on nothing else. A second past the
/// cutoff, so the boundary above isn't a second reason for this to fail.
#[tokio::test]
async fn a_recorded_miss_names_exactly_the_artists_it_was_given() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artist_a: i64 = sqlx::query_scalar("SELECT id FROM artists WHERE name = 'Artist A'")
        .fetch_one(db.read())
        .await?;

    queries::artist::mark_image_search_missed(&db, &[artist_a], CUTOFF + 1).await?;

    let still_searched: Vec<String> = queries::artist::get_artists_without_images(&db, CUTOFF)
        .await?
        .into_iter()
        .map(|artist| artist.name)
        .collect();
    assert_eq!(still_searched, ["Artist B"]);
    Ok(())
}

/// Every batch of an image pass records its misses, most of them none.
#[tokio::test]
async fn marking_no_artist_writes_nothing() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    queries::artist::mark_image_search_missed(&db, &[], CUTOFF).await?;

    let marked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artists WHERE image_search_missed_at IS NOT NULL")
            .fetch_one(db.read())
            .await?;
    assert_eq!(marked, 0);
    Ok(())
}

#[tokio::test]
async fn update_artist_image_path() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = listed_artists(&db).await?;
    let artist = artists
        .iter()
        .find(|a| a.name == "Artist A")
        .ok_or_else(|| AppError::validation_msg("Artist A missing"))?;

    queries::artist::update_artist_image_path(&db, artist.id, "/images/artist_a.jpg").await?;

    let updated = queries::artist::get_artist_by_id(&db, artist.id).await?;
    assert_eq!(updated.image_path.as_deref(), Some("/images/artist_a.jpg"));

    // Should no longer appear in "without images"
    let without = queries::artist::get_artists_without_images(&db, i64::MAX).await?;
    assert!(!without.iter().any(|a| a.id == artist.id));
    Ok(())
}

/// The count arrives through a `COUNT(t.id) AS favorite_count` alias, and that is the half of
/// this projection only a real fetch can check: `FavoriteArtist` matches columns by name, so a
/// renamed alias fails at fetch time and nowhere earlier.
///
/// One artist favorited and one not, because the `JOIN … AND t.is_favorite = TRUE` is what
/// keeps the untouched artist out; a fixture where everything is favorited passes with the
/// condition deleted.
#[tokio::test]
async fn a_favorited_artist_carries_the_count_of_its_favorited_tracks() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let artists = listed_artists(&db).await?;
    let artist_a = artists
        .iter()
        .find(|a| a.name == "Artist A")
        .ok_or_else(|| AppError::validation_msg("Artist A missing"))?;

    // Both of Artist A's tracks, and neither of Artist B's.
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM tracks WHERE title IN ('Alpha Song', 'Gamma Song')")
            .fetch_all(db.read())
            .await?;
    queries::track::set_favorite(&db, &ids, true).await?;

    let favorites = listed_favorite_artists(&db).await?;

    assert_eq!(
        favorites,
        vec![FavoriteArtist {
            id: artist_a.id,
            name: "Artist A".to_owned(),
            image_path: None,
            favorite_count: 2,
        }]
    );
    Ok(())
}

#[tokio::test]
async fn a_library_with_no_favorites_has_no_favorite_artists() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    assert!(listed_favorite_artists(&db).await?.is_empty());
    Ok(())
}
