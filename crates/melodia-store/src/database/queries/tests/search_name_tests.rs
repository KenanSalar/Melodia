//! The backfill against the writers it stands in for: a row it fills has to read as the writer
//! would have left it, or a library filled on upgrade filters differently from one filled by scans.

use std::path::Path;

use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::artist::UNKNOWN_ARTIST_ID;
use crate::database::queries::fixtures::{insert_tagged_track, make_test_metadata};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

fn track(title: &str, artist: &str, album: &str, genre: &str) -> ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.artist = ArtistCredit::from_name(artist);
    meta.album = Some(album.to_owned());
    meta.genres = GenreList::from_name(genre);
    meta
}

async fn add(db: &DbPool, meta: &ExtractedMetadata) -> Result<i64, AppError> {
    let path = Path::new("/music").join(format!("{}.mp3", meta.title));
    insert_tagged_track(db, &path.to_string_lossy(), meta).await
}

/// Every statement that writes one of the four names, once each: the scan's inserts, an album
/// credit a later track widens, a sort name, both playlist inserts, and a rename down each of its
/// two arms.
async fn written_library() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    add(&db, &track("One", "Solo", "Plain", "Rock")).await?;
    add(&db, &track("Two", "Solo", "Duets", "jazz")).await?;
    let mut widened = track("Three", "Solo", "Duets", "jazz");
    widened.album_artist =
        ArtistCredit::from_tags("Solo & Guest", &["Solo".to_owned(), "Guest".to_owned()]);
    add(&db, &widened).await?;
    let mut sorted = track("Four", "Zed", "Late", "Pop");
    sorted.sort.artist = Some("Omega, Zed".to_owned());
    add(&db, &sorted).await?;

    queries::playlist::create_playlist(&db, "Road Trip", None).await?;
    queries::playlist::create_smart_playlist(&db, "Focus", None, "{}").await?;
    for (before, after, clear_thumbnail) in [("Before", "After", false), ("Was", "Now", true)] {
        let id = queries::playlist::create_playlist(&db, before, None).await?.id;
        queries::playlist::update_playlist(&db, id, after, None, clear_thumbnail).await?;
    }
    Ok(db)
}

/// Every row's `search_name` across the four tables, bar the seeded unknown artist's: no writer
/// touches that row, so only the backfill ever fills it.
async fn search_names(db: &DbPool) -> Result<Vec<(String, i64, String)>, AppError> {
    let rows = sqlx::query_as(
        "SELECT 'albums', id, search_name FROM albums \
         UNION ALL SELECT 'artists', id, search_name FROM artists WHERE id <> ? \
         UNION ALL SELECT 'genres', id, search_name FROM genres \
         UNION ALL SELECT 'playlists', id, search_name FROM playlists \
         ORDER BY 1, 2",
    )
    .bind(UNKNOWN_ARTIST_ID)
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// The state the migration leaves an existing library in.
async fn blank_every_search_name(db: &DbPool) -> Result<(), AppError> {
    for table in ["albums", "artists", "genres", "playlists"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {table} SET search_name = ''")))
            .execute(db.write())
            .await?;
    }
    Ok(())
}

/// One equality holds both halves: a writer that skips the column, or leaves an older name in it,
/// differs from what the backfill reads off the row, and so does a backfill that packs a kind's
/// fields differently from its writers.
#[tokio::test]
async fn the_backfill_writes_what_every_writer_wrote() -> Result<(), AppError> {
    let db = written_library().await?;
    let written = search_names(&db).await?;
    blank_every_search_name(&db).await?;

    super::backfill_search_name(&db).await?;

    assert_eq!(search_names(&db).await?, written);
    Ok(())
}
