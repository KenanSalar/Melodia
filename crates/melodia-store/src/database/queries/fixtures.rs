//! Seeded rows for the suites that need a library to query.
//!
//! `#[doc(hidden)] pub` rather than `#[cfg(test)]`, and not by preference: `library`'s and
//! `tasks`' tests seed through these, and a `cfg(test)` item cannot cross a crate boundary.
//! [`DbPool::test_pool`] is the same shape for the same reason, and `lto = "fat"` is what makes
//! either cost the shipped binary nothing.

use crate::database::DbPool;
use crate::database::queries;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

/// Create a default `ExtractedMetadata` with sensible test values.
/// Override fields as needed after calling this.
pub fn make_test_metadata(title: &str) -> ExtractedMetadata {
    ExtractedMetadata {
        title: title.to_owned(),
        artist: Some("Test Artist".to_owned()),
        album_artist: None,
        album: Some("Test Album".to_owned()),
        genre: Some("Rock".to_owned()),
        track_number: Some(1),
        disc_number: Some(1),
        year: Some(2024),
        composer: None,
        comment: None,
        bpm: None,
        musicbrainz_track_id: None,
        musicbrainz_release_id: None,
        label: None,
        original_year: None,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
        rating: None,
        duration_ms: 180_000,
        codec: Some("Mpeg".to_owned()),
        bitrate: Some(320),
        channels: Some(2),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        file_size: 5_000_000,
        file_hash: blake3::hash(title.as_bytes()).to_hex().to_string(),
        date_modified: Some("2024-01-01T00:00:00+00:00".to_owned()),
        artwork_path: None,
    }
}

/// Insert a test track with full scan workflow (upsert artist/album/genre + insert).
/// Returns the track ID.
pub async fn insert_test_track(
    db: &DbPool,
    file_path: &str,
    title: &str,
    artist_name: &str,
    album_name: &str,
    genre_name: &str,
) -> Result<i64, AppError> {
    let mut tx = db.write().begin().await?;

    let unknown_artist_id = 1; // sentinel from schema.sql
    let artist_id = queries::scan::upsert_artist(&mut tx, artist_name, unknown_artist_id).await?;
    let album_id = queries::scan::upsert_album(&mut tx, album_name, artist_id, Some(2024)).await?;
    let genre_id = queries::scan::upsert_genre(&mut tx, genre_name).await?;

    let file_name =
        std::path::Path::new(file_path).file_name().and_then(|f| f.to_str()).unwrap_or("test.mp3");

    let mut meta = make_test_metadata(title);
    meta.artist = if artist_name.is_empty() {
        None
    } else {
        Some(artist_name.to_owned())
    };
    meta.album = if album_name.is_empty() {
        None
    } else {
        Some(album_name.to_owned())
    };
    meta.genre = if genre_name.is_empty() {
        None
    } else {
        Some(genre_name.to_owned())
    };

    let ids = queries::ResolvedIds {
        artist_id,
        album_id,
        genre_id,
        folder_id: 1, // default folder
    };

    let now = melodia_core::utils::now_rfc3339();
    queries::scan::insert_track(&mut tx, file_path, file_name, &meta, &ids, &now).await?;

    tx.commit().await?;

    // Retrieve the track ID
    let id: i64 = sqlx::query_scalar::<_, i64>("SELECT id FROM tracks WHERE file_path = ?")
        .bind(file_path)
        .fetch_one(db.read())
        .await?;
    Ok(id)
}

/// Create a test pool pre-seeded with a folder and 3 tracks.
pub async fn setup_seeded_db() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;

    // Insert a folder
    queries::folder::insert_folder(&db, "/music", true).await?;

    // Insert 3 tracks
    insert_test_track(&db, "/music/track1.mp3", "Alpha Song", "Artist A", "Album One", "Rock")
        .await?;
    insert_test_track(&db, "/music/track2.mp3", "Beta Song", "Artist B", "Album Two", "Pop")
        .await?;
    insert_test_track(&db, "/music/track3.mp3", "Gamma Song", "Artist A", "Album One", "Rock")
        .await?;

    Ok(db)
}
