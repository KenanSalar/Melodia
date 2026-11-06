use super::{FILL_PAGE_ROWS, fill_missing};
use crate::database::queries::fixtures::{
    execute, insert_tagged_track, make_test_metadata, numbered_library,
};
use crate::database::queries::folder::insert_folder;
use crate::database::{DbPool, sql};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::error::AppError;

/// Every row back to how a library written before the keys holds it.
const FORGET_KEYS: &str = "UPDATE tracks SET artist_key = NULL, album_key = NULL, genre_key = NULL";

async fn keys_of(db: &DbPool, id: i64) -> Result<(String, String, String), AppError> {
    db.read(move |conn| {
        sql::fetch_one(
            conn,
            "SELECT artist_key, album_key, genre_key FROM tracks WHERE id = ?",
            (id,),
        )
    })
    .await
}

/// The key is what `UNICODE_LOWER` compares, Rust's lowercase of the whole string: a final sigma
/// folds to `ς`, which a character-at-a-time lowercase would miss.
#[tokio::test]
async fn the_fill_keys_a_row_by_its_lowercased_fields() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    insert_folder(&db, "/music", true).await?;
    let mut meta = make_test_metadata("One");
    meta.artist = ArtistCredit::from_name("Björk");
    meta.album = Some("ΟΔΟΣ".to_owned());
    meta.genres = GenreList::default();
    let id = insert_tagged_track(&db, "/music/1.mp3", &meta).await?;
    execute(&db, FORGET_KEYS).await?;

    fill_missing(&db).await?;

    assert_eq!(keys_of(&db, id).await?, ("björk".to_owned(), "οδος".to_owned(), String::new()));
    Ok(())
}

#[tokio::test]
async fn the_fill_reaches_the_rows_past_its_first_page() -> Result<(), AppError> {
    let rows = usize::try_from(FILL_PAGE_ROWS).unwrap_or(usize::MAX) + 1;
    let (db, _) = numbered_library(rows).await?;
    execute(&db, FORGET_KEYS).await?;

    fill_missing(&db).await?;

    let unkeyed: i64 = db
        .read(|conn| {
            sql::fetch_one(conn, "SELECT COUNT(*) FROM tracks WHERE artist_key IS NULL", ())
        })
        .await?;
    assert_eq!(unkeyed, 0);
    Ok(())
}

/// The check runs on every open, so a keyed library has to answer it with nothing to write, an
/// untagged track's `''` included.
#[tokio::test]
async fn the_fill_writes_nothing_once_every_row_is_keyed() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    insert_folder(&db, "/music", true).await?;
    let mut untagged = make_test_metadata("Untagged");
    untagged.artist = ArtistCredit::default();
    untagged.album = None;
    untagged.genres = GenreList::default();
    insert_tagged_track(&db, "/music/tagged.mp3", &make_test_metadata("Tagged")).await?;
    insert_tagged_track(&db, "/music/untagged.mp3", &untagged).await?;

    let filled = fill_missing(&db).await?;

    assert_eq!(filled, 0);
    Ok(())
}
