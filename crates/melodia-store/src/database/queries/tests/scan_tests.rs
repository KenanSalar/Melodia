use std::path::Path;

use rusqlite::Connection;

use super::upserts;
use crate::database::queries;
use crate::database::queries::fixtures::*;
use crate::database::queries::scan::NameCache;
use crate::database::{DbPool, sql};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::credits::{CreditRole, RoleCredit, RoleCredits};
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

/// The release side of what `upsert_album` reads, carrying nothing but the year.
///
/// It takes the whole `ExtractedMetadata` now rather than a bare year, the release-level tags
/// having moved onto `albums` — and every one of them defaults to absent, which is the state these
/// cases were already asserting against.
fn album_meta(year: Option<i32>) -> ExtractedMetadata {
    let mut meta = make_test_metadata("Track");
    meta.year = year;
    meta
}

// === Pure unit tests for to_natural_sort_key ===

#[test]
fn sort_key_pads_numbers() {
    assert_eq!(queries::scan::to_natural_sort_key("Track 2"), "track 00000002");
}

#[test]
fn sort_key_leading_digits() {
    assert_eq!(queries::scan::to_natural_sort_key("10 Songs"), "00000010 songs");
}

#[test]
fn sort_key_no_digits() {
    assert_eq!(queries::scan::to_natural_sort_key("abc"), "abc");
}

#[test]
fn sort_key_empty() {
    assert_eq!(queries::scan::to_natural_sort_key(""), "");
}

#[test]
fn sort_key_mixed_alpha_numeric() {
    assert_eq!(queries::scan::to_natural_sort_key("a1b2c3"), "a00000001b00000002c00000003");
}

#[test]
fn sort_key_large_number_not_truncated() {
    assert_eq!(queries::scan::to_natural_sort_key("track 123456789"), "track 123456789");
}

#[test]
fn sort_key_preserves_ordering() {
    let mut keys: Vec<String> = vec!["Track 10", "Track 2", "Track 1", "Track 20"]
        .into_iter()
        .map(queries::scan::to_natural_sort_key)
        .collect();
    keys.sort();
    assert_eq!(keys[0], queries::scan::to_natural_sort_key("Track 1"));
    assert_eq!(keys[1], queries::scan::to_natural_sort_key("Track 2"));
    assert_eq!(keys[2], queries::scan::to_natural_sort_key("Track 10"));
    assert_eq!(keys[3], queries::scan::to_natural_sort_key("Track 20"));
}

// === Async DB tests ===

#[tokio::test]
async fn track_exists_by_path_false_when_empty() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let exists =
        db.transaction(|tx| queries::scan::track_exists_by_path(tx, "/nonexistent.mp3")).await?;
    assert!(!exists);
    Ok(())
}

#[tokio::test]
async fn track_exists_by_path_true_after_insert() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    let exists =
        db.transaction(|tx| queries::scan::track_exists_by_path(tx, "/music/song.mp3")).await?;
    assert!(exists);
    Ok(())
}

#[tokio::test]
async fn upsert_artist_new_returns_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = db.transaction(|tx| upserts::upsert_artist(tx, "New Artist", 1)).await?;
    assert!(id > 1); // 1 is the sentinel "Unknown Artist"
    Ok(())
}

#[tokio::test]
async fn upsert_artist_duplicate_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let (id1, id2) = db
        .transaction(|tx| {
            let id1 = upserts::upsert_artist(tx, "Duplicate", 1)?;
            let id2 = upserts::upsert_artist(tx, "Duplicate", 1)?;
            Ok((id1, id2))
        })
        .await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn upsert_artist_empty_returns_unknown() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = db.transaction(|tx| upserts::upsert_artist(tx, "", 1)).await?;
    assert_eq!(id, 1);
    Ok(())
}

#[tokio::test]
async fn upsert_album_new_returns_some() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let album_id = db
        .transaction(|tx| {
            let credit = ArtistCredit::from_name("Artist");
            let artist_id = upserts::upsert_artist(tx, "Artist", 1)?;
            queries::scan::upsert_album(
                tx,
                "Album",
                artist_id,
                &credit,
                &album_meta(Some(2024)),
                &mut NameCache::default(),
            )
        })
        .await?;
    assert!(album_id.is_some());
    Ok(())
}

#[tokio::test]
async fn upsert_album_duplicate_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let (id1, id2) = db
        .transaction(|tx| {
            let credit = ArtistCredit::from_name("Artist");
            let artist_id = upserts::upsert_artist(tx, "Artist", 1)?;
            let meta = album_meta(Some(2024));
            let mut names = NameCache::default();
            let id1 =
                queries::scan::upsert_album(tx, "Album", artist_id, &credit, &meta, &mut names)?;
            let id2 =
                queries::scan::upsert_album(tx, "Album", artist_id, &credit, &meta, &mut names)?;
            Ok((id1, id2))
        })
        .await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn upsert_album_empty_returns_none() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let result = db
        .transaction(|tx| {
            let credit = ArtistCredit::default();
            queries::scan::upsert_album(
                tx,
                "",
                1,
                &credit,
                &album_meta(None),
                &mut NameCache::default(),
            )
        })
        .await?;
    assert!(result.is_none());
    Ok(())
}

#[tokio::test]
async fn upsert_album_updates_year_on_conflict() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let (id, same, updated, kept) = db
        .transaction(|tx| {
            let credit = ArtistCredit::from_name("Artist");
            let artist_id = upserts::upsert_artist(tx, "Artist", 1)?;
            let mut names = NameCache::default();
            let id = queries::scan::upsert_album(
                tx,
                "Album",
                artist_id,
                &credit,
                &album_meta(Some(2001)),
                &mut names,
            )?;
            let album_id = id.ok_or_else(|| AppError::validation_msg("no album id"))?;

            // Re-upsert of the same (name, artist_id) with a new year updates it.
            let same = queries::scan::upsert_album(
                tx,
                "Album",
                artist_id,
                &credit,
                &album_meta(Some(2010)),
                &mut names,
            )?;
            let updated = album_year(tx, album_id)?;

            // Re-upsert with a NULL year preserves the stored value (the COALESCE arm).
            queries::scan::upsert_album(
                tx,
                "Album",
                artist_id,
                &credit,
                &album_meta(None),
                &mut names,
            )?;
            Ok((id, same, updated, album_year(tx, album_id)?))
        })
        .await?;
    assert_eq!(id, same);
    assert_eq!(updated, Some(2010));
    assert_eq!(kept, Some(2010));
    Ok(())
}

fn album_year(conn: &Connection, album_id: i64) -> Result<Option<i32>, AppError> {
    sql::fetch_one(conn, "SELECT year FROM albums WHERE id = ?", (album_id,))
}

#[tokio::test]
async fn upsert_genre_new_returns_some() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let genre_id = db.transaction(|tx| upserts::upsert_genre(tx, "Rock")).await?;
    assert!(genre_id.is_some());
    Ok(())
}

#[tokio::test]
async fn upsert_genre_duplicate_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let (id1, id2) = db
        .transaction(|tx| {
            Ok((upserts::upsert_genre(tx, "Rock")?, upserts::upsert_genre(tx, "Rock")?))
        })
        .await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn upsert_genre_empty_returns_none() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let result = db.transaction(|tx| upserts::upsert_genre(tx, "")).await?;
    assert!(result.is_none());
    Ok(())
}

#[tokio::test]
async fn insert_track_stores_correct_fields() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let now = "2024-01-01T00:00:00+00:00";
    db.transaction(move |tx| {
        let credit = ArtistCredit::from_name("Test Artist");
        let artist_id = upserts::upsert_artist(tx, "Test Artist", 1)?;
        let meta = make_test_metadata("My Song");
        let mut names = NameCache::default();
        let album_id =
            queries::scan::upsert_album(tx, "Test Album", artist_id, &credit, &meta, &mut names)?;
        let genre_id = upserts::upsert_genre(tx, "Rock")?;
        let ids = queries::ResolvedIds { artist_id, album_id, genre_id, folder_id: 1 };
        queries::scan::insert_track(tx, "/music/my.mp3", "my.mp3", &meta, &ids, now, &mut names)
    })
    .await?;

    // Verify via raw query
    let row: (String, i64, String) = db
        .read(|conn| {
            sql::fetch_one(
                conn,
                "SELECT title, duration_ms, date_added FROM tracks \
                 WHERE file_path = '/music/my.mp3'",
                (),
            )
        })
        .await?;
    assert_eq!(row.0, "My Song");
    assert_eq!(row.1, 180_000);
    assert_eq!(row.2, now);
    Ok(())
}

#[tokio::test]
async fn update_album_artwork_from_tracks_fills_missing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;
    set_test_artwork(&db, id, "/art/cover.jpg").await?;

    roll_up_album_covers(&db).await?;

    assert_eq!(album_artwork(&db).await?.as_deref(), Some("/art/cover.jpg"));
    Ok(())
}

async fn album_artwork(db: &DbPool) -> Result<Option<String>, AppError> {
    db.read(|conn| sql::fetch_one(conn, "SELECT artwork_path FROM albums WHERE name = 'Album'", ()))
        .await
}

async fn roll_up_album_covers(db: &DbPool) -> Result<(), AppError> {
    db.transaction(queries::scan::update_album_artwork_from_tracks).await
}

/// A track with no cover sits ahead of the ones that have one, so a lookup that took the album's
/// first track rather than its first covered one would leave the album blank.
#[tokio::test]
async fn an_album_takes_the_cover_of_its_lowest_id_track_that_has_one() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/1.mp3", "One", "Artist", "Album", "Rock").await?;
    let two = insert_test_track(&db, "/music/2.mp3", "Two", "Artist", "Album", "Rock").await?;
    let three = insert_test_track(&db, "/music/3.mp3", "Three", "Artist", "Album", "Rock").await?;
    set_test_artwork(&db, three, "/art/three.jpg").await?;
    set_test_artwork(&db, two, "/art/two.jpg").await?;

    roll_up_album_covers(&db).await?;

    assert_eq!(album_artwork(&db).await?.as_deref(), Some("/art/two.jpg"));
    Ok(())
}

/// Null-only: a cover the user picked for the album outlives any scan.
#[tokio::test]
async fn an_album_cover_already_set_is_never_overwritten() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/1.mp3", "One", "Artist", "Album", "Rock").await?;
    set_test_artwork(&db, id, "/art/track.jpg").await?;
    db.write(|conn| {
        sql::execute(
            conn,
            "UPDATE albums SET artwork_path = '/art/chosen.jpg' WHERE name = 'Album'",
            (),
        )
    })
    .await?;

    roll_up_album_covers(&db).await?;

    assert_eq!(album_artwork(&db).await?.as_deref(), Some("/art/chosen.jpg"));
    Ok(())
}

/// The reason the two roll-ups are one call: a scan that changed the cover a playlisted track
/// holds and refreshed only the albums would leave the playlist naming a cover nothing holds.
#[tokio::test]
async fn rolling_up_covers_moves_an_automatic_playlist_onto_its_first_tracks_cover()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/1.mp3", "One", "Artist", "Album", "Rock").await?;
    set_test_artwork(&db, id, "/art/old.jpg").await?;
    let playlist = queries::playlist::create_playlist(&db, "Mix", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, playlist.id, vec![id]).await?;
    set_test_artwork(&db, id, "/art/rescanned.jpg").await?;

    db.transaction(queries::scan::roll_up_covers).await?;

    let shown = queries::playlist::get_playlist_by_id(&db, playlist.id).await?.thumbnail_path;
    assert_eq!(shown.as_deref(), Some("/art/rescanned.jpg"));
    Ok(())
}

// === Tests for delete_track_by_path ===

#[tokio::test]
async fn delete_track_by_path_returns_true_when_exists() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    let deleted =
        db.transaction(|tx| queries::scan::delete_track_by_path(tx, "/music/song.mp3")).await?;

    assert!(deleted);

    // Verify track is gone
    assert_eq!(track_count(&db).await?, 0);
    Ok(())
}

#[tokio::test]
async fn delete_track_by_path_returns_false_when_not_found() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let deleted =
        db.transaction(|tx| queries::scan::delete_track_by_path(tx, "/nonexistent.mp3")).await?;
    assert!(!deleted);
    Ok(())
}

async fn track_count(db: &DbPool) -> Result<i64, AppError> {
    db.read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM tracks", ())).await
}

// === Tests for delete_tracks_by_paths_batch ===

#[tokio::test]
async fn delete_tracks_batch_deletes_multiple() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    let paths = vec!["/music/track1.mp3".to_owned(), "/music/track3.mp3".to_owned()];
    let deleted =
        db.transaction(move |tx| queries::scan::delete_tracks_by_paths_batch(tx, &paths)).await?;

    assert_eq!(deleted, 2);
    assert_eq!(track_count(&db).await?, 1); // only track2 remains
    Ok(())
}

#[tokio::test]
async fn delete_tracks_batch_empty_returns_zero() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let deleted = db.transaction(|tx| queries::scan::delete_tracks_by_paths_batch(tx, &[])).await?;
    assert_eq!(deleted, 0);
    Ok(())
}

// === Tests for find_folder_for_path ===

#[tokio::test]
async fn find_folder_for_path_matches_parent() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let folder_id =
        db.transaction(|tx| queries::scan::find_folder_for_path(tx, "/music/song.mp3")).await?;
    assert!(folder_id.is_some());
    Ok(())
}

/// Every other test here spells a POSIX path, which is every path on the CI runner and none
/// on Windows — where a library folder is `C:\Music` and its tracks `C:\Music\a.mp3`. Building
/// the pair from `MAIN_SEPARATOR_STR` asks the question each platform actually faces; a literal
/// backslash would only ever fail on Linux, which is why the gap survived.
#[tokio::test]
async fn find_folder_for_path_matches_a_native_separator() -> Result<(), AppError> {
    use std::path::MAIN_SEPARATOR_STR as SEP;

    let db = DbPool::test_pool().await?;
    let folder = format!("{SEP}music");
    queries::folder::insert_folder(&db, &folder, true).await?;

    let song = format!("{folder}{SEP}song.mp3");
    let folder_id =
        db.transaction(move |tx| queries::scan::find_folder_for_path(tx, &song)).await?;
    assert!(folder_id.is_some(), "a path spelled the way the OS spells it must resolve");
    Ok(())
}

#[tokio::test]
async fn find_folder_for_path_matches_nested() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    queries::folder::insert_folder(&db, "/music/rock", true).await?;

    let rock_id: i64 = db
        .read(|conn| sql::fetch_one(conn, "SELECT id FROM folders WHERE path = '/music/rock'", ()))
        .await?;

    // Should match the longer prefix "/music/rock"
    let folder_id = db
        .transaction(|tx| queries::scan::find_folder_for_path(tx, "/music/rock/song.mp3"))
        .await?;
    assert_eq!(folder_id, Some(rock_id));
    Ok(())
}

#[tokio::test]
async fn find_folder_for_path_returns_none_for_unknown() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let folder_id =
        db.transaction(|tx| queries::scan::find_folder_for_path(tx, "/other/song.mp3")).await?;
    assert!(folder_id.is_none());
    Ok(())
}

// === Tests for fold_folder_tracks ===

#[tokio::test]
async fn fold_folder_tracks_hands_over_every_row_of_the_folder() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    let paths = queries::scan::fold_folder_tracks(&db, 1, Vec::new(), |paths, path, _stamp| {
        paths.push(path);
    })
    .await?;

    assert_eq!(paths.len(), 3);
    assert!(paths.contains(&"/music/track1.mp3".to_owned()));
    assert!(paths.contains(&"/music/track2.mp3".to_owned()));
    assert!(paths.contains(&"/music/track3.mp3".to_owned()));
    Ok(())
}

/// The scan purges every row it is handed that its walk didn't find, so a row leaking in from
/// another folder is deleted by it. The second folder's path starts with the first's.
#[tokio::test]
async fn fold_folder_tracks_hands_over_no_row_of_another_folder() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let sibling = queries::folder::insert_folder(&db, "/music-extra", true).await?;
    let stray = "/music-extra/track4.mp3";
    insert_test_track(&db, stray, "Delta Song", "Artist C", "Album Three", "Jazz").await?;
    db.write(move |conn| {
        sql::execute(
            conn,
            "UPDATE tracks SET folder_id = ? WHERE file_path = ?",
            (sibling.id, stray),
        )
    })
    .await?;

    let mut paths = queries::scan::fold_folder_tracks(&db, 1, Vec::new(), |paths, path, _stamp| {
        paths.push(path);
    })
    .await?;

    paths.sort_unstable();
    assert_eq!(paths, ["/music/track1.mp3", "/music/track2.mp3", "/music/track3.mp3"]);
    Ok(())
}

/// What the incremental filter compares a file against. Another column read in their place reads
/// every file as changed, and a launch re-parses the whole library.
#[tokio::test]
async fn fold_folder_tracks_hands_each_row_its_stored_size_and_mtime() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/a.mp3", "Song", "Artist", "Album", "Rock").await?;

    let rows = queries::scan::fold_folder_tracks(&db, 1, Vec::new(), |rows, _path, stamp| {
        rows.push((stamp.file_size, stamp.date_modified));
    })
    .await?;

    let stored = make_test_metadata("Song");
    assert_eq!(rows, [(Some(stored.file_size), stored.date_modified)]);
    Ok(())
}

#[tokio::test]
async fn fold_folder_tracks_folds_nothing_for_an_unknown_folder() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let folded =
        queries::scan::fold_folder_tracks(&db, 999, 0, |folded, _path, _stamp| *folded += 1)
            .await?;
    assert_eq!(folded, 0);
    Ok(())
}

// === Tests for get_track_id_by_path ===

#[tokio::test]
async fn get_track_id_by_path_returns_id_when_exists() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let expected_id =
        insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    let id =
        db.transaction(|tx| queries::scan::get_track_id_by_path(tx, "/music/song.mp3")).await?;
    assert_eq!(id, Some(expected_id));
    Ok(())
}

#[tokio::test]
async fn get_track_id_by_path_returns_none_when_missing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id =
        db.transaction(|tx| queries::scan::get_track_id_by_path(tx, "/nonexistent.mp3")).await?;
    assert!(id.is_none());
    Ok(())
}

// === Tests for update_track_location ===

#[tokio::test]
async fn update_track_location_repoints_existing_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let folder = queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/old.mp3", "Song", "Artist", "Album", "Rock").await?;

    let (repointed, moved) = db
        .transaction(move |tx| {
            let repointed = queries::scan::update_track_location(
                tx,
                id,
                "/music/new.mp3",
                "new.mp3",
                folder.id,
                None,
            )?;
            Ok((repointed, queries::scan::get_track_id_by_path(tx, "/music/new.mp3")?))
        })
        .await?;
    assert!(repointed);
    assert_eq!(moved, Some(id));
    Ok(())
}

#[tokio::test]
async fn update_track_location_false_when_row_deleted_in_tx() -> Result<(), AppError> {
    // The watcher batch resolves its move candidates before the write
    // transaction opens, so a scan can commit a delete of the candidate row
    // in between. The re-point must report "no row hit" so `handle_created`
    // falls back to a fresh insert instead of silently dropping the track.
    let db = DbPool::test_pool().await?;
    let folder = queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/old.mp3", "Song", "Artist", "Album", "Rock").await?;

    let (deleted, repointed) = db
        .transaction(move |tx| {
            let deleted = queries::scan::delete_track_by_path(tx, "/music/old.mp3")?;
            let repointed = queries::scan::update_track_location(
                tx,
                id,
                "/music/new.mp3",
                "new.mp3",
                folder.id,
                None,
            )?;
            Ok((deleted, repointed))
        })
        .await?;
    assert!(deleted);
    assert!(!repointed);
    Ok(())
}

// === The credit tables the scan writes beside a track ===

/// A track credited as `printed` to `credited`, which is what the join rows are built from.
fn credited(title: &str, printed: &str, names: &[&str]) -> ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.artist = ArtistCredit::from_tags(
        printed,
        &names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>(),
    );
    meta
}

/// A track's artist join rows as (position, name, join phrase).
async fn track_artists(db: &DbPool, track_id: i64) -> Result<Vec<(i64, String, String)>, AppError> {
    db.read(move |conn| {
        sql::fetch_all(
            conn,
            "SELECT ta.position, a.name, ta.join_phrase \
             FROM track_artists ta JOIN artists a ON a.id = ta.artist_id \
             WHERE ta.track_id = ? ORDER BY ta.position",
            (track_id,),
        )
    })
    .await
}

/// **Position 0 is the row's own `tracks.artist_id`.** Album grouping and the sort indexes still
/// read that FK, so the join table agreeing with it at the head is what stops an artist-scoped
/// query and an album-scoped one disagreeing about the same track.
#[tokio::test]
async fn a_credit_writes_one_row_per_name_with_the_primary_at_the_head() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let meta = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    let id = insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    assert_eq!(
        track_artists(&db, id).await?,
        vec![(0, "Alice".to_owned(), " feat. ".to_owned()), (1, "Bob".to_owned(), String::new()),]
    );

    let head: i64 = db
        .read(move |conn| {
            sql::fetch_one(
                conn,
                "SELECT CASE WHEN t.artist_id = ta.artist_id THEN 1 ELSE 0 END \
                 FROM tracks t JOIN track_artists ta ON ta.track_id = t.id AND ta.position = 0 \
                 WHERE t.id = ?",
                (id,),
            )
        })
        .await?;
    assert_eq!(head, 1, "position 0 has to be the row's own artist_id");
    Ok(())
}

/// **One row per artist, not per name.** `artists.name` is `UNIQUE COLLATE NOCASE`, so a credit
/// naming someone twice under two spellings resolves to one row — and the stats triggers on this
/// table would then count the track twice for them.
#[tokio::test]
async fn a_name_spelled_twice_in_one_credit_is_joined_once() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let meta = credited("One", "Bob & bob", &["Bob", "bob"]);
    let id = insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    assert_eq!(track_artists(&db, id).await?.len(), 1);

    let bob: i64 = db
        .read(|conn| sql::fetch_one(conn, "SELECT track_count FROM artists WHERE name = 'Bob'", ()))
        .await?;
    assert_eq!(bob, 1, "the join rows are what a count reads");
    // The printed line keeps both, being what the release spelled.
    assert_eq!(meta.artist.line(), Some("Bob & bob"));
    Ok(())
}

/// A file with no artist tag resolves to the sentinel and still gets its row, rather than being
/// credited to nobody and dropping out of every artist-scoped query.
#[tokio::test]
async fn a_track_crediting_nobody_still_gets_a_join_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let mut meta = make_test_metadata("One");
    meta.artist = ArtistCredit::default();
    let id = insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    assert_eq!(track_artists(&db, id).await?.len(), 1);
    Ok(())
}

/// **The three tables are one fact about the row**, so a re-ingest replaces all of them together —
/// a path that cleared two is a track displaying credits it cannot be found by. Driven through
/// `update_track_metadata`, which is the door a tag edit and a re-scan both come through.
#[tokio::test]
async fn re_ingesting_a_track_leaves_none_of_its_old_join_rows() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut before = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    before.genres = GenreList::new(vec!["Rock".to_owned(), "Metal".to_owned()]);
    before.credits = RoleCredits::new(vec![RoleCredit {
        role: CreditRole::Composer,
        name: "Carol".to_owned(),
        detail: String::new(),
    }]);
    let id = insert_tagged_track(&db, "/music/1.mp3", &before).await?;

    let mut after = credited("One", "Alice", &[]);
    after.genres = GenreList::from_name("Rock");
    db.transaction(move |tx| {
        let mut names = queries::scan::NameCache::default();
        let ids = queries::ResolvedIds {
            artist_id: names.artist(tx, "Alice", 1)?,
            album_id: None,
            genre_id: names.genre(tx, "Rock")?,
            folder_id: 1,
        };
        queries::scan::update_track_metadata(tx, "/music/1.mp3", &after, &ids, &mut names)
    })
    .await?;

    let counts: (i64, i64, i64) = db
        .read(move |conn| {
            sql::fetch_one(
                conn,
                "SELECT (SELECT COUNT(*) FROM track_artists WHERE track_id = ?), \
                        (SELECT COUNT(*) FROM track_genres WHERE track_id = ?), \
                        (SELECT COUNT(*) FROM track_credits WHERE track_id = ?)",
                (id, id, id),
            )
        })
        .await?;
    assert_eq!(counts, (1, 1, 0));
    Ok(())
}

async fn search_text_of(db: &DbPool, id: i64) -> Result<String, AppError> {
    db.read(move |conn| sql::fetch_one(conn, "SELECT search_text FROM tracks WHERE id = ?", (id,)))
        .await
}

/// **A new row is searchable the moment it lands**, its five fields folded into the column the
/// library-sized lists filter on, with no backfill involved.
#[tokio::test]
async fn an_inserted_track_carries_its_folded_search_text() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let mut meta = make_test_metadata("Jóga");
    meta.artist = ArtistCredit::from_name("Björk");
    meta.album_artist = ArtistCredit::from_name("Björk");
    meta.album = Some("Homogenic".to_owned());
    meta.genres = GenreList::from_name("Electronic");
    let path = Path::new("/music").join("joga.flac");

    let id = insert_tagged_track(&db, &path.to_string_lossy(), &meta).await?;

    assert_eq!(search_text_of(&db, id).await?, "joga\nbjork\nbjork\nhomogenic\nelectronic");
    Ok(())
}

/// **A re-ingest refolds the row**, which is what keeps a tag edit findable under its new title:
/// the column is already filled, so the backfill would never look at it again.
#[tokio::test]
async fn a_re_ingest_refolds_the_search_text() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let path = Path::new("/music").join("1.mp3").to_string_lossy().into_owned();
    let id = insert_tagged_track(&db, &path, &make_test_metadata("Before")).await?;

    let mut after = make_test_metadata("Après");
    after.artist = ArtistCredit::from_name("Alice");
    db.transaction(move |tx| {
        let mut names = queries::scan::NameCache::default();
        let ids = queries::ResolvedIds {
            artist_id: names.artist(tx, "Alice", 1)?,
            album_id: None,
            genre_id: names.genre(tx, "Rock")?,
            folder_id: 1,
        };
        queries::scan::update_track_metadata(tx, &path, &after, &ids, &mut names)
    })
    .await?;

    assert_eq!(search_text_of(&db, id).await?, "apres\nalice\n\ntest album\nrock");
    Ok(())
}

async fn order_keys_of(db: &DbPool, id: i64) -> Result<(String, String, String), AppError> {
    db.read(move |conn| {
        sql::fetch_one(
            conn,
            "SELECT artist_key, album_key, genre_key FROM tracks WHERE id = ?",
            (id,),
        )
    })
    .await
}

/// **A new row sorts by its own keys from the moment it lands**: a NULL key sorts first until the
/// next launch's fill reaches it.
#[tokio::test]
async fn an_inserted_track_carries_its_lowercase_order_keys() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let mut meta = make_test_metadata("Jóga");
    meta.artist = ArtistCredit::from_name("Björk");
    meta.album = None;
    meta.genres = GenreList::from_name("Électronique");
    let path = Path::new("/music").join("joga.flac");

    let id = insert_tagged_track(&db, &path.to_string_lossy(), &meta).await?;

    let keys = ("björk".to_owned(), String::new(), "électronique".to_owned());
    assert_eq!(order_keys_of(&db, id).await?, keys);
    Ok(())
}

/// **A re-ingest rekeys the row**, which is what moves a tag edit to where its new artist sorts:
/// the keys are already filled, so the fill would never look at it again.
#[tokio::test]
async fn a_re_ingest_rekeys_the_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let path = Path::new("/music").join("1.mp3").to_string_lossy().into_owned();
    let id = insert_tagged_track(&db, &path, &make_test_metadata("Before")).await?;

    let mut after = make_test_metadata("After");
    after.artist = ArtistCredit::from_name("ΟΔΟΣ");
    after.album = Some("Homogenic".to_owned());
    after.genres = GenreList::default();
    db.transaction(move |tx| {
        let mut names = queries::scan::NameCache::default();
        let ids = queries::ResolvedIds {
            artist_id: names.artist(tx, "ΟΔΟΣ", 1)?,
            album_id: None,
            genre_id: None,
            folder_id: 1,
        };
        queries::scan::update_track_metadata(tx, &path, &after, &ids, &mut names)
    })
    .await?;

    let keys = ("οδος".to_owned(), "homogenic".to_owned(), String::new());
    assert_eq!(order_keys_of(&db, id).await?, keys);
    Ok(())
}

// === Where an album files itself ===

/// **The whole track credit is the wrong key.** A guest on one track is not an album artist, so
/// taking "X feat. Y" here would rename the album after whichever track reached the upsert first
/// and list it in Y's discography.
#[test]
fn an_album_files_under_the_primary_name_and_not_the_whole_credit() {
    let mut meta = make_test_metadata("One");
    meta.artist =
        ArtistCredit::from_tags("Alice feat. Bob", &["Alice".to_owned(), "Bob".to_owned()]);

    assert_eq!(queries::scan::album_artist_name_for(&meta), "Alice");
    assert_eq!(queries::scan::album_credit_for(&meta).line(), Some("Alice"));
}

/// Its own tag wins where there is one, which is what keeps a compilation together.
#[test]
fn an_album_artist_tag_outranks_the_track_credit() {
    let mut meta = make_test_metadata("One");
    meta.artist = ArtistCredit::from_name("Alice");
    meta.album_artist = ArtistCredit::from_name("Various Artists");

    assert_eq!(queries::scan::album_artist_name_for(&meta), "Various Artists");
    assert_eq!(queries::scan::album_credit_for(&meta).line(), Some("Various Artists"));
}

#[test]
fn an_album_with_nothing_to_file_under_credits_nobody() {
    let mut meta = make_test_metadata("One");
    meta.artist = ArtistCredit::default();
    meta.album_artist = ArtistCredit::default();

    assert_eq!(queries::scan::album_artist_name_for(&meta), "");
    assert!(queries::scan::album_credit_for(&meta).is_empty());
}

/// **A one-name credit is left NULL rather than repeating the name it would.** `albums` has only
/// ever had the FK, so `album_stats` falls back to the artist row — and rendering both is how the
/// two would come to disagree about a name a rescan changed on one of them.
#[tokio::test]
async fn an_album_renders_only_a_credit_the_artist_row_cannot_state() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let solo = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    insert_tagged_track(&db, "/music/1.mp3", &solo).await?;

    let mut shared = make_test_metadata("Two");
    shared.album = Some("Split".to_owned());
    shared.album_artist =
        ArtistCredit::from_tags("Alice & Bob", &["Alice".to_owned(), "Bob".to_owned()]);
    insert_tagged_track(&db, "/music/2.mp3", &shared).await?;

    // The track's guest is not an album artist, so the album files under the primary alone.
    assert_eq!(album_credit(&db, "Test Album").await?, ("Alice".to_owned(), None));
    assert_eq!(
        album_credit(&db, "Split").await?,
        ("Alice".to_owned(), Some("Alice & Bob".to_owned()))
    );
    Ok(())
}

/// An album's filed-under artist and the credit it renders, if any.
async fn album_credit(db: &DbPool, name: &str) -> Result<(String, Option<String>), AppError> {
    let name = name.to_owned();
    db.read(move |conn| {
        sql::fetch_one(
            conn,
            "SELECT a.name, al.artist_credit \
             FROM albums al JOIN artists a ON a.id = al.artist_id WHERE al.name = ?",
            (name,),
        )
    })
    .await
}

// === The name cache ===

/// One upsert per distinct spelling per transaction. A guest repeats across a release and the join
/// writers ask once per credit per track, so without it a 200-track box set is 200 identical
/// upserts of the same name.
#[tokio::test]
async fn a_name_asked_for_twice_resolves_to_one_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let (first, again) = db
        .transaction(|tx| {
            let mut names = queries::scan::NameCache::for_chunk(10);
            let first = names.artist(tx, "Alice", 1)?;
            let again = names.artist(tx, "Alice", 1)?;
            Ok((first, again))
        })
        .await?;

    assert_eq!(first, again);
    let rows: i64 = db
        .read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM artists WHERE name = 'Alice'", ()))
        .await?;
    assert_eq!(rows, 1);
    Ok(())
}

/// **Keyed on the exact spelling**, so two spellings share one row and the upsert leaves the
/// newest standing — and a spelling seen *again* after a different one no longer wins the row
/// back, which is how the primary-artist path has always behaved.
#[tokio::test]
async fn a_spelling_seen_again_after_another_does_not_win_the_row_back() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    db.transaction(|tx| {
        let mut names = queries::scan::NameCache::default();
        names.artist(tx, "Alice", 1)?;
        names.artist(tx, "ALICE", 1)?;
        names.artist(tx, "Alice", 1)?;
        Ok(())
    })
    .await?;

    let stored: (i64, String) = db
        .read(|conn| {
            sql::fetch_one(
                conn,
                "SELECT COUNT(*), MAX(name) FROM artists WHERE name = 'alice' COLLATE NOCASE",
                (),
            )
        })
        .await?;
    assert_eq!(stored, (1, "ALICE".to_owned()));
    Ok(())
}

/// An empty name answers the sentinel without touching the table, so callers stay branch-free.
#[tokio::test]
async fn a_name_with_nothing_in_it_resolves_to_the_sentinel() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let (artist, genre) = db
        .transaction(|tx| {
            let mut names = queries::scan::NameCache::default();
            Ok((names.artist(tx, "", 1)?, names.genre(tx, "")?))
        })
        .await?;

    assert_eq!(artist, 1);
    assert_eq!(genre, None);
    let rows: i64 =
        db.read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM artists", ())).await?;
    assert_eq!(rows, 1, "only the schema's own sentinel row");
    Ok(())
}

/// **The credit tables are part of what keeps an artist alive.** A predicate over the two FKs
/// alone deletes exactly the artists the credit tables exist to surface.
#[tokio::test]
async fn a_guest_who_is_nobodys_primary_artist_survives_the_prune() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let meta = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    db.transaction(queries::scan::prune_orphans).await?;

    let bob: i64 = db
        .read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM artists WHERE name = 'Bob'", ()))
        .await?;
    assert_eq!(bob, 1);
    Ok(())
}
