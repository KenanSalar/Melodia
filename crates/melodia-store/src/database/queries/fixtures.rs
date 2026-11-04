//! Seeded rows for the suites that need a library to query, and the literal SQL they seed and read
//! back with where no store function does.
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
use sqlx::sqlite::{Sqlite, SqliteRow};
use sqlx::{AssertSqlSafe, Row};
// For the `listed_*` helpers below, which only this crate's suites call.
#[cfg(test)]
use {
    melodia_core::entities::album::AlbumStats,
    melodia_core::entities::artist::{ArtistStats, FavoriteArtist},
    melodia_core::entities::genre::GenreStats,
    melodia_core::entities::grid_order::{AlbumSort, ArtistSort, FavoriteArtistSort, GenreSort},
    melodia_core::entities::track_list::SortDir,
    melodia_core::utils::needle::Needle,
};

/// A default `ExtractedMetadata` with test values, to override field by field.
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

/// Inserts a test track through the scan's own upserts, returning its id.
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

/// Inserts a track from a prepared [`ExtractedMetadata`], resolving its rows as
/// `queries::scan::resolve_track_context` does, for the guest credits, second genres and role
/// credits [`insert_test_track`]'s flat arguments can't carry. `folder_id` is the seeded 1.
pub async fn insert_tagged_track(
    db: &DbPool,
    file_path: &str,
    meta: &ExtractedMetadata,
) -> Result<i64, AppError> {
    let mut tx = db.write().begin().await?;
    let mut names = queries::scan::NameCache::default();

    let artist_name = meta.artist.primary_name();
    let artist_id = names.artist(&mut tx, artist_name, queries::artist::UNKNOWN_ARTIST_ID).await?;
    let album_artist_name = queries::scan::album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        names.artist(&mut tx, album_artist_name, queries::artist::UNKNOWN_ARTIST_ID).await?
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

/// Points a track's cover at `path`, which the seeding helpers leave null.
pub async fn set_test_artwork(db: &DbPool, track_id: i64, path: &str) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;
    queries::track::set_track_artwork(&mut tx, &[track_id], Some(path)).await?;
    tx.commit().await?;
    Ok(())
}

/// A pool holding `count` tracks, in insert order, for the suites that need more rows than one
/// statement's bind budget covers. Each is titled by its index, so no two share a hash.
#[cfg(test)]
pub(crate) async fn numbered_library(count: usize) -> Result<(DbPool, Vec<i64>), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let path = format!("/music/{index:04}.mp3");
        let title = format!("Track {index:04}");
        ids.push(insert_test_track(&db, &path, &title, "Artist", "Album", "Rock").await?);
    }
    Ok((db, ids))
}

/// Keeps the file at `path` out of the library as the duplicate merge does, carrying `file_hash`
/// and [`make_test_metadata`]'s stamp. `folder_id` is the seeded 1.
#[cfg(test)]
pub(crate) async fn keep_out(db: &DbPool, path: &str, file_hash: &str) -> Result<(), AppError> {
    let stamp = make_test_metadata("");
    let exclusion = queries::excluded_files::NewExclusion {
        path,
        file_hash: Some(file_hash),
        file_size: Some(stamp.file_size),
        date_modified: stamp.date_modified.as_deref(),
        folder_id: 1,
    };
    let mut tx = db.write().begin().await?;
    queries::excluded_files::exclude_on(&mut tx, &[exclusion]).await?;
    tx.commit().await?;
    Ok(())
}

/// The cards the Albums grid lists unfiltered, by name.
#[cfg(test)]
pub(crate) async fn listed_albums(db: &DbPool) -> Result<Vec<AlbumStats>, AppError> {
    let ids =
        queries::entity_keyset::album_ids(db, AlbumSort::Name, SortDir::Asc, &Needle::default())
            .await?;
    queries::album::get_albums_by_ids(db, &ids).await
}

/// The cards the Artists grid lists unfiltered, by name.
#[cfg(test)]
pub(crate) async fn listed_artists(db: &DbPool) -> Result<Vec<ArtistStats>, AppError> {
    let ids =
        queries::entity_keyset::artist_ids(db, ArtistSort::Name, SortDir::Asc, &Needle::default())
            .await?;
    queries::artist::get_artists_by_ids(db, &ids).await
}

/// The cards the Genres grid lists unfiltered, by name.
#[cfg(test)]
pub(crate) async fn listed_genres(db: &DbPool) -> Result<Vec<GenreStats>, AppError> {
    let ids =
        queries::entity_keyset::genre_ids(db, GenreSort::Name, SortDir::Asc, &Needle::default())
            .await?;
    queries::genre::get_genres_by_ids(db, &ids).await
}

/// The cards Favorites' Favorite Artists grid lists unfiltered, by name.
#[cfg(test)]
pub(crate) async fn listed_favorite_artists(db: &DbPool) -> Result<Vec<FavoriteArtist>, AppError> {
    let ids = queries::entity_keyset::favorite_artist_ids(
        db,
        FavoriteArtistSort::Name,
        SortDir::Asc,
        &Needle::default(),
    )
    .await?;
    queries::artist::get_favorite_artists_by_ids(db, &ids).await
}

/// Create a test pool pre-seeded with a folder and 3 tracks.
pub async fn setup_seeded_db() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    insert_test_track(&db, "/music/track1.mp3", "Alpha Song", "Artist A", "Album One", "Rock")
        .await?;
    insert_test_track(&db, "/music/track2.mp3", "Beta Song", "Artist B", "Album Two", "Pop")
        .await?;
    insert_test_track(&db, "/music/track3.mp3", "Gamma Song", "Artist A", "Album One", "Rock")
        .await?;

    Ok(db)
}

/// Runs `sql` on the writer, for a state no store function seeds. Values are written into the
/// text, [`quoted`] where they are text, so a suite outside the store never names the driver.
pub async fn execute(db: &DbPool, sql: &str) -> Result<(), AppError> {
    sqlx::query(AssertSqlSafe(sql)).execute(db.write()).await?;
    Ok(())
}

/// The one row `sql` answers, read on a reader.
pub async fn fetch_one<T: Readback>(db: &DbPool, sql: &str) -> Result<T, AppError> {
    let row = sqlx::query(AssertSqlSafe(sql)).fetch_one(db.read()).await?;
    Ok(T::from_row(&row)?)
}

/// The row `sql` answers, if it answers one.
pub async fn fetch_optional<T: Readback>(db: &DbPool, sql: &str) -> Result<Option<T>, AppError> {
    let row = sqlx::query(AssertSqlSafe(sql)).fetch_optional(db.read()).await?;
    Ok(row.as_ref().map(T::from_row).transpose()?)
}

/// Every row `sql` answers.
pub async fn fetch_all<T: Readback>(db: &DbPool, sql: &str) -> Result<Vec<T>, AppError> {
    let rows = sqlx::query(AssertSqlSafe(sql)).fetch_all(db.read()).await?;
    Ok(rows.iter().map(T::from_row).collect::<Result<_, _>>()?)
}

/// `text` as an SQL string literal, for a value [`execute`] or a read writes into its statement.
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// What a read decodes each row into: one column, or a tuple of them in select order.
pub trait Readback: Sized {
    fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error>;
}

macro_rules! column_readback {
    ($($column:ty),+) => {$(
        impl Readback for $column {
            fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
                row.try_get::<Self, usize>(0)
            }
        }
    )+};
}

column_readback!(i64, Option<i64>, String, Option<String>, bool);

macro_rules! tuple_readback {
    ($($column:ident: $index:literal),+) => {
        impl<$($column),+> Readback for ($($column,)+)
        where
            $($column: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,)+
        {
            fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
                Ok(($(row.try_get::<$column, usize>($index)?,)+))
            }
        }
    };
}

tuple_readback!(A: 0, B: 1);
tuple_readback!(A: 0, B: 1, C: 2);
tuple_readback!(A: 0, B: 1, C: 2, D: 3);
tuple_readback!(A: 0, B: 1, C: 2, D: 3, E: 4);
