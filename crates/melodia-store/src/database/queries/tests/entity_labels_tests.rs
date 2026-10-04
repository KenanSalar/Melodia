//! The names a card's Copy entry resolves its ids to.
//!
//! Each statement is built at run time, so a table or column spelled wrong compiles and fails only
//! on the first copy of that kind. One test per kind is what reaches all four.

use crate::database::DbPool;
use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::error::AppError;

/// An id no fixture reaches.
const ABSENT_ID: i64 = 1_000_000;

async fn library() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    Ok(db)
}

async fn id_named(db: &DbPool, table: &str, name: &str) -> Result<i64, AppError> {
    let id = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {table} WHERE name = ?"
    )))
    .bind(name)
    .fetch_one(db.read())
    .await?;
    Ok(id)
}

/// A multi-name credit is the case that tells `album_stats` apart from the album's own artist
/// row, which holds only the first name.
#[tokio::test]
async fn an_album_label_credits_the_artist_line_the_album_grid_prints() -> Result<(), AppError> {
    let db = library().await?;
    let mut meta = make_test_metadata("Opening");
    meta.album = Some("Duets".to_owned());
    meta.album_artist =
        ArtistCredit::from_tags("First & Second", &["First".to_owned(), "Second".to_owned()]);
    insert_tagged_track(&db, "/music/opening.mp3", &meta).await?;
    let album = id_named(&db, "albums", "Duets").await?;

    let labels = queries::entity_labels::album_labels(&db, &[album]).await?;

    assert_eq!(labels, [(album, "Duets".to_owned(), "First & Second".to_owned())]);
    Ok(())
}

/// A card deleted between the grid painting and the click.
#[tokio::test]
async fn an_id_with_no_row_contributes_no_label() -> Result<(), AppError> {
    let db = library().await?;
    insert_test_track(&db, "/music/a.mp3", "A", "Solo", "Album", "Rock").await?;
    let artist = id_named(&db, "artists", "Solo").await?;

    let names = queries::entity_labels::artist_names(&db, &[ABSENT_ID, artist]).await?;

    assert_eq!(names, [(artist, "Solo".to_owned())]);
    Ok(())
}

#[tokio::test]
async fn a_genre_answers_with_its_name() -> Result<(), AppError> {
    let db = library().await?;
    insert_test_track(&db, "/music/a.mp3", "A", "Solo", "Album", "Shoegaze").await?;
    let genre = id_named(&db, "genres", "Shoegaze").await?;

    let names = queries::entity_labels::genre_names(&db, &[genre]).await?;

    assert_eq!(names, [(genre, "Shoegaze".to_owned())]);
    Ok(())
}

#[tokio::test]
async fn a_playlist_answers_with_its_name() -> Result<(), AppError> {
    let db = library().await?;
    let playlist = queries::playlist::create_playlist(&db, "Road Trip", None).await?;

    let names = queries::entity_labels::playlist_names(&db, &[playlist.id]).await?;

    assert_eq!(names, [(playlist.id, "Road Trip".to_owned())]);
    Ok(())
}
