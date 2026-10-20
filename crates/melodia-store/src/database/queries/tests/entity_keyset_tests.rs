//! Every card grid's keyset, held to the order and the matches of the in-memory sort and filter it
//! replaced, written out per arm over fixtures built from the partitions SQL could answer
//! differently.

use std::path::Path;

use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::entity_keyset;
use crate::database::queries::fixtures::{insert_tagged_track, make_test_metadata};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::grid_order::{AlbumSort, ArtistSort, FavoriteArtistSort, GenreSort};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::track_list::SortDir;
use melodia_core::error::AppError;
use melodia_core::utils::needle::{Needle, fold_needle};

async fn library() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    Ok(db)
}

/// A track titled `title` by `artist` on `album`. Titles name the file and seed its hash, so no
/// two in one fixture may share one.
fn track(title: &str, artist: &str, album: &str) -> ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.artist = ArtistCredit::from_name(artist);
    meta.album = Some(album.to_owned());
    meta
}

fn credit(line: &str, names: &[&str]) -> ArtistCredit {
    let names: Vec<String> = names.iter().map(|&name| name.to_owned()).collect();
    ArtistCredit::from_tags(line, &names)
}

async fn add(db: &DbPool, meta: &ExtractedMetadata) -> Result<i64, AppError> {
    let path = Path::new("/music").join(format!("{}.mp3", meta.title));
    insert_tagged_track(db, &path.to_string_lossy(), meta).await
}

/// The name behind each of `ids`, in order, read off `table` itself. A card fetch would drop an id
/// it has no card for, hiding exactly the id a grid should never have listed.
async fn names(db: &DbPool, table: &str, ids: &[i64]) -> Result<Vec<String>, AppError> {
    let sql = format!("SELECT name FROM {table} WHERE id = ?");
    let mut names = Vec::with_capacity(ids.len());
    for &id in ids {
        let name = sqlx::query_scalar(AssertSqlSafe(sql.as_str())).bind(id).fetch_one(db.read());
        names.push(name.await?);
    }
    Ok(names)
}

// === Albums ===

/// An accented case pair that lowercases to one name, so only the id parts it; lowercase names
/// beside capitalised ones, which `BINARY` puts last; a missing year beside an explicit `0` and
/// named to sort after it, where a bare `NULL` would sort first; and a shared credit whose line
/// sorts past its primary artist's name.
async fn albums_library() -> Result<DbPool, AppError> {
    let db = library().await?;
    let staged = [
        ("One", "émile", "Mid", Some(1999)),
        ("Two", "Émile", "Mid", Some(1999)),
        ("Three", "apple", "Zed", Some(0)),
        ("Four", "Banana", "alpha", None),
    ];
    for (title, album, artist, year) in staged {
        let mut meta = track(title, artist, album);
        meta.year = year;
        add(&db, &meta).await?;
    }
    let mut shared = track("Five", "Mid", "Cherry");
    shared.album_artist = credit("Mid feat. Guest", &["Mid", "Guest"]);
    shared.year = Some(2001);
    add(&db, &shared).await?;
    Ok(db)
}

async fn album_names(
    db: &DbPool,
    sort: AlbumSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<String>, AppError> {
    let ids = entity_keyset::album_ids(db, sort, dir, needle).await?;
    names(db, "albums", &ids).await
}

#[tokio::test]
async fn the_albums_grid_sorts_by_every_arm_both_ways() -> Result<(), AppError> {
    let db = albums_library().await?;
    let cases: [(AlbumSort, SortDir, [&str; 5]); 6] = [
        (AlbumSort::Name, SortDir::Asc, ["apple", "Banana", "Cherry", "émile", "Émile"]),
        (AlbumSort::Name, SortDir::Desc, ["Émile", "émile", "Cherry", "Banana", "apple"]),
        (AlbumSort::Year, SortDir::Asc, ["apple", "Banana", "émile", "Émile", "Cherry"]),
        (AlbumSort::Year, SortDir::Desc, ["Cherry", "Émile", "émile", "Banana", "apple"]),
        (AlbumSort::Artist, SortDir::Asc, ["Banana", "émile", "Émile", "Cherry", "apple"]),
        (AlbumSort::Artist, SortDir::Desc, ["apple", "Cherry", "Émile", "émile", "Banana"]),
    ];

    for (sort, dir, expected) in cases {
        assert_eq!(
            album_names(&db, sort, dir, &Needle::default()).await?,
            expected,
            "Albums by {sort:?} {}",
            dir.as_str()
        );
    }
    Ok(())
}

/// The artist half matches the line the card draws, so a guest on a shared credit finds the
/// album, which a join on the primary artist alone would miss.
#[tokio::test]
async fn the_albums_filter_matches_the_name_the_artist_line_and_the_year() -> Result<(), AppError> {
    let db = albums_library().await?;
    let cases: [(&str, &[&str]); 5] = [
        ("EMILE", &["émile", "Émile"]),
        ("guest", &["Cherry"]),
        ("mid", &["Cherry", "émile", "Émile"]),
        ("199", &["émile", "Émile"]),
        ("zzz", &[]),
    ];

    for (raw, expected) in cases {
        assert_eq!(
            album_names(&db, AlbumSort::Name, SortDir::Asc, &fold_needle(raw)).await?,
            expected,
            "needle {raw:?}"
        );
    }
    Ok(())
}

// === Artists ===

/// Three artists whose name, track-count and album-count orders all differ, each count arm tying
/// a pair only the name parts, one with a sort name its name doesn't carry, and a guest credited
/// on an album and on no track.
async fn artists_library() -> Result<DbPool, AppError> {
    let db = library().await?;
    let staged = [
        ("A1", "alpha", "A", None),
        ("A2", "alpha", "B", None),
        ("M1", "Mid", "C", None),
        ("M2", "Mid", "D", None),
        ("Z1", "Zed", "F", Some("Omega, Zed")),
        ("Z2", "Zed", "F", Some("Omega, Zed")),
    ];
    for (title, artist, album, sort_name) in staged {
        let mut meta = track(title, artist, album);
        meta.sort.artist = sort_name.map(str::to_owned);
        add(&db, &meta).await?;
    }
    let mut shared = track("M3", "Mid", "E");
    shared.album_artist = credit("Mid feat. Guest", &["Mid", "Guest"]);
    add(&db, &shared).await?;
    Ok(db)
}

async fn artist_names(
    db: &DbPool,
    sort: ArtistSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<String>, AppError> {
    let ids = entity_keyset::artist_ids(db, sort, dir, needle).await?;
    names(db, "artists", &ids).await
}

#[tokio::test]
async fn the_artists_grid_sorts_by_every_arm_both_ways() -> Result<(), AppError> {
    let db = artists_library().await?;
    let cases: [(ArtistSort, SortDir, [&str; 3]); 6] = [
        (ArtistSort::Name, SortDir::Asc, ["alpha", "Mid", "Zed"]),
        (ArtistSort::Name, SortDir::Desc, ["Zed", "Mid", "alpha"]),
        (ArtistSort::TrackCount, SortDir::Asc, ["alpha", "Zed", "Mid"]),
        (ArtistSort::TrackCount, SortDir::Desc, ["Mid", "Zed", "alpha"]),
        (ArtistSort::AlbumCount, SortDir::Asc, ["Zed", "alpha", "Mid"]),
        (ArtistSort::AlbumCount, SortDir::Desc, ["Mid", "alpha", "Zed"]),
    ];

    for (sort, dir, expected) in cases {
        assert_eq!(
            artist_names(&db, sort, dir, &Needle::default()).await?,
            expected,
            "Artists by {sort:?} {}",
            dir.as_str()
        );
    }
    Ok(())
}

/// The seeded unknown artist holds no tracks and an unfilled `search_name`, which matches every
/// needle, so only the grid's membership keeps it out; the guest holds no tracks and a name the
/// needle does carry.
#[tokio::test]
async fn the_artists_filter_matches_the_name_and_the_sort_name() -> Result<(), AppError> {
    let db = artists_library().await?;
    let cases: [(&str, &[&str]); 4] =
        [("ALPHA", &["alpha"]), ("omega", &["Zed"]), ("guest", &[]), ("unknown", &[])];

    for (raw, expected) in cases {
        assert_eq!(
            artist_names(&db, ArtistSort::Name, SortDir::Asc, &fold_needle(raw)).await?,
            expected,
            "needle {raw:?}"
        );
    }
    Ok(())
}

// === Genres ===

/// Three genres whose name, track-count and duration orders all differ, a lowercase name among
/// them, and a track-count tie only the name parts.
async fn genres_library() -> Result<DbPool, AppError> {
    let db = library().await?;
    let staged = [
        ("J1", "jazz", 100),
        ("J2", "jazz", 100),
        ("P1", "Pop", 300),
        ("R1", "Rock", 50),
        ("R2", "Rock", 50),
    ];
    for (title, genre, duration_ms) in staged {
        let mut meta = track(title, "Artist", "Album");
        meta.genres = GenreList::from_name(genre);
        meta.duration_ms = duration_ms;
        add(&db, &meta).await?;
    }
    Ok(db)
}

#[tokio::test]
async fn the_genres_grid_sorts_by_every_arm_both_ways() -> Result<(), AppError> {
    let db = genres_library().await?;
    let cases: [(GenreSort, SortDir, [&str; 3]); 6] = [
        (GenreSort::Name, SortDir::Asc, ["jazz", "Pop", "Rock"]),
        (GenreSort::Name, SortDir::Desc, ["Rock", "Pop", "jazz"]),
        (GenreSort::TrackCount, SortDir::Asc, ["Pop", "jazz", "Rock"]),
        (GenreSort::TrackCount, SortDir::Desc, ["Rock", "jazz", "Pop"]),
        (GenreSort::Duration, SortDir::Asc, ["Rock", "jazz", "Pop"]),
        (GenreSort::Duration, SortDir::Desc, ["Pop", "jazz", "Rock"]),
    ];

    for (sort, dir, expected) in cases {
        let ids = entity_keyset::genre_ids(&db, sort, dir, &Needle::default()).await?;
        assert_eq!(
            names(&db, "genres", &ids).await?,
            expected,
            "Genres by {sort:?} {}",
            dir.as_str()
        );
    }
    Ok(())
}

// === Favorite Artists ===

/// A guest credited on a favourite, a favourite-count tie only the name parts, a track that is no
/// favourite beside an artist's favourites, an artist with none, and a sort name.
async fn favorites_library() -> Result<DbPool, AppError> {
    let db = library().await?;
    let mut featuring = track("F2", "alpha", "Album");
    featuring.artist = credit("alpha feat. Mid", &["alpha", "Mid"]);
    let mut zed_one = track("F4", "Zed", "Album");
    zed_one.sort.artist = Some("Omega, Zed".to_owned());
    let mut zed_two = track("F5", "Zed", "Album");
    zed_two.sort.artist = Some("Omega, Zed".to_owned());
    let staged = [
        (track("F1", "alpha", "Album"), true),
        (featuring, true),
        (track("F3", "alpha", "Album"), false),
        (zed_one, true),
        (zed_two, true),
        (track("F6", "Quiet", "Album"), false),
    ];
    for (meta, favorite) in staged {
        let id = add(&db, &meta).await?;
        queries::track::set_favorite(&db, &[id], favorite).await?;
    }
    Ok(db)
}

async fn favorite_artist_names(
    db: &DbPool,
    sort: FavoriteArtistSort,
    dir: SortDir,
    needle: &Needle,
) -> Result<Vec<String>, AppError> {
    let ids = entity_keyset::favorite_artist_ids(db, sort, dir, needle).await?;
    names(db, "artists", &ids).await
}

#[tokio::test]
async fn the_favorite_artists_grid_sorts_by_every_arm_both_ways() -> Result<(), AppError> {
    let db = favorites_library().await?;
    let cases: [(FavoriteArtistSort, SortDir, [&str; 3]); 4] = [
        (FavoriteArtistSort::FavoriteCount, SortDir::Asc, ["Mid", "alpha", "Zed"]),
        (FavoriteArtistSort::FavoriteCount, SortDir::Desc, ["Zed", "alpha", "Mid"]),
        (FavoriteArtistSort::Name, SortDir::Asc, ["alpha", "Mid", "Zed"]),
        (FavoriteArtistSort::Name, SortDir::Desc, ["Zed", "Mid", "alpha"]),
    ];

    for (sort, dir, expected) in cases {
        assert_eq!(
            favorite_artist_names(&db, sort, dir, &Needle::default()).await?,
            expected,
            "Favorite Artists by {sort:?} {}",
            dir.as_str()
        );
    }
    Ok(())
}

/// Matching the sort name is the one deliberate change from the grid this replaced, which matched
/// the name alone: it reads the column the Artists grid filters by.
#[tokio::test]
async fn the_favorite_artists_filter_matches_the_sort_name_too() -> Result<(), AppError> {
    let db = favorites_library().await?;
    let cases: [(&str, &[&str]); 3] = [("MID", &["Mid"]), ("omega", &["Zed"]), ("quiet", &[])];

    for (raw, expected) in cases {
        assert_eq!(
            favorite_artist_names(&db, FavoriteArtistSort::Name, SortDir::Asc, &fold_needle(raw))
                .await?,
            expected,
            "needle {raw:?}"
        );
    }
    Ok(())
}

// === Playlists ===

/// A tie on the instant goes to the newer playlist, the id reversing with the time, so the order
/// is total.
#[tokio::test]
async fn the_playlists_grid_lists_the_newest_change_first() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let changed = [
        ("Spring", "2026-03-01T00:00:00+00:00"),
        ("Summer", "2026-06-01T00:00:00+00:00"),
        ("Spring again", "2026-03-01T00:00:00+00:00"),
    ];
    for (name, updated_at) in changed {
        let id = queries::playlist::create_playlist(&db, name, None).await?.id;
        sqlx::query("UPDATE playlists SET updated_at = ? WHERE id = ?")
            .bind(updated_at)
            .bind(id)
            .execute(db.write())
            .await?;
    }

    let ids = entity_keyset::playlist_ids(&db, &Needle::default()).await?;

    assert_eq!(names(&db, "playlists", &ids).await?, ["Summer", "Spring again", "Spring"]);
    Ok(())
}
