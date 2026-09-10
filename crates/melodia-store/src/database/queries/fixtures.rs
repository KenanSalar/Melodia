//! Seeded rows for the suites that need a library to query.
//!
//! `#[doc(hidden)] pub` rather than `#[cfg(test)]`, and not by preference: `library`'s and
//! `tasks`' tests seed through these, and a `cfg(test)` item cannot cross a crate boundary.
//! [`DbPool::test_pool`] is the same shape for the same reason, and `lto = "fat"` is what makes
//! either cost the shipped binary nothing.

use crate::database::DbPool;
use crate::database::queries;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::credits::RoleCredits;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::{ExtractedMetadata, ReleaseTags, SortTags};
use melodia_core::error::AppError;

/// Create a default `ExtractedMetadata` with sensible test values.
/// Override fields as needed after calling this.
pub fn make_test_metadata(title: &str) -> ExtractedMetadata {
    ExtractedMetadata {
        title: title.to_owned(),
        artist: ArtistCredit::from_name("Test Artist"),
        album_artist: ArtistCredit::default(),
        artist_mbids: Vec::new(),
        album_artist_mbids: Vec::new(),
        album: Some("Test Album".to_owned()),
        genres: GenreList::from_name("Rock"),
        credits: RoleCredits::default(),
        sort: SortTags::default(),
        release: ReleaseTags::default(),
        track_number: Some(1),
        track_total: None,
        disc_number: Some(1),
        disc_total: None,
        disc_subtitle: None,
        subtitle: None,
        release_date: None,
        year: Some(2024),
        original_date: None,
        original_year: None,
        comment: None,
        bpm: None,
        initial_key: None,
        mood: None,
        grouping: None,
        work: None,
        movement: None,
        movement_number: None,
        movement_total: None,
        language: None,
        copyright: None,
        isrc: None,
        musicbrainz_track_id: None,
        musicbrainz_release_id: None,
        musicbrainz_release_track_id: None,
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
    let mut meta = make_test_metadata(title);
    meta.artist = ArtistCredit::from_name(artist_name);
    meta.album = if album_name.is_empty() { None } else { Some(album_name.to_owned()) };
    meta.genres = GenreList::from_name(genre_name);

    insert_tagged_track(db, file_path, &meta).await
}

/// Insert a track from a prepared [`ExtractedMetadata`], resolving its rows the way
/// `queries::scan::resolve_track_context` does.
///
/// The multi-value half of a tag — a guest credit, a second genre, a role credit — has no shape in
/// [`insert_test_track`]'s flat arguments, and the join tables every stat is now counted off are
/// written from exactly that half. Everything but the folder is the scan's own path; `folder_id`
/// is the seeded 1, since no fixture stages a path a library folder actually contains.
pub async fn insert_tagged_track(
    db: &DbPool,
    file_path: &str,
    meta: &ExtractedMetadata,
) -> Result<i64, AppError> {
    let unknown_artist_id = 1; // sentinel from schema.sql
    let mut tx = db.write().begin().await?;
    let mut names = queries::scan::NameCache::default();

    let artist_name = meta.artist.primary_name();
    let artist_id = names.artist(&mut tx, artist_name, unknown_artist_id).await?;
    let album_artist_name = queries::scan::album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        names.artist(&mut tx, album_artist_name, unknown_artist_id).await?
    };
    let album_credit = queries::scan::album_credit_for(meta);
    let album_id = queries::scan::upsert_album(
        &mut tx,
        meta.album.as_deref().unwrap_or(""),
        album_artist_id,
        &album_credit,
        meta,
        &mut names,
    )
    .await?;
    let genre_id = names.genre(&mut tx, meta.genres.primary().unwrap_or("")).await?;

    let file_name =
        std::path::Path::new(file_path).file_name().and_then(|f| f.to_str()).unwrap_or("test.mp3");

    let ids = queries::ResolvedIds {
        artist_id,
        album_id,
        genre_id,
        folder_id: 1, // default folder
    };

    let now = melodia_core::utils::now_rfc3339();
    let id =
        queries::scan::insert_track(&mut tx, file_path, file_name, meta, &ids, &now, &mut names)
            .await?;
    tx.commit().await?;
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
