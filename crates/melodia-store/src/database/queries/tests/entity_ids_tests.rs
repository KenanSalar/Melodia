//! What a card's right-click action queues, and in what order.
//!
//! **Every fixture here is inserted back to front**, `track_tests.rs`'s
//! `seed_db_inserted_backwards` lesson: seeded in the order being asserted, a bare table scan
//! satisfies the assertion and the `ORDER BY` can go missing with nothing failing. Where the
//! wrong clause is a *specific* other clause, the fixture is also built so that one disagrees:
//! the artist and genre arms would pass against `sort_key` if their titles and albums agreed.
//!
//! Which clause each query owes agreement to is `crates/melodia/tests/cross_tier.rs`'s question.
//! These pin that the clause written here is the one that runs.

use std::path::Path;

use crate::database::DbPool;
use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

const LIBRARY_ROOT: &str = "/music";

/// An id range no fixture reaches, for padding a chunk out past its bind budget.
const ABSENT_ENTITY_ID: i64 = 1_000_000;

async fn empty_library() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, LIBRARY_ROOT, true).await?;
    Ok(db)
}

/// One track, keyed on its title: the path, the hash and the natural sort key all follow from it,
/// so a fixture cannot accidentally share any of the three.
async fn stage(db: &DbPool, meta: &ExtractedMetadata) -> Result<i64, AppError> {
    let path = Path::new(LIBRARY_ROOT).join(format!("{}.mp3", meta.title));
    insert_tagged_track(db, &path.to_string_lossy(), meta).await
}

/// `title` under `album`, at `disc` / `track`. `None` for either number is the untagged partition
/// the `COALESCE` pair exists for.
fn numbered(title: &str, album: &str, disc: Option<i32>, track: Option<i32>) -> ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.album = Some(album.to_owned());
    meta.disc_number = disc;
    meta.track_number = track;
    meta
}

async fn album_id(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM albums WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(id)
}

async fn artist_id(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM artists WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(id)
}

async fn genre_id(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM genres WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(id)
}

/// Titles for the track ids a query answered with, so a failure names the tracks rather than
/// whatever rowids the fixture happened to get.
async fn titles_of(db: &DbPool, track_ids: &[i64]) -> Result<Vec<String>, AppError> {
    let mut titles = Vec::with_capacity(track_ids.len());
    for id in track_ids {
        titles.push(
            sqlx::query_scalar::<_, String>("SELECT title FROM tracks WHERE id = ?")
                .bind(id)
                .fetch_one(db.read())
                .await?,
        );
    }
    Ok(titles)
}

fn track_ids(pairs: &[(i64, i64)]) -> Vec<i64> {
    pairs.iter().map(|&(_, track_id)| track_id).collect()
}

#[tokio::test]
async fn album_tracks_come_back_in_disc_then_track_order() -> Result<(), AppError> {
    let db = empty_library().await?;
    stage(&db, &numbered("Second Disc", "Boxed", Some(2), Some(1))).await?;
    stage(&db, &numbered("Later", "Boxed", Some(1), Some(2))).await?;
    stage(&db, &numbered("Opener", "Boxed", Some(1), Some(1))).await?;

    let pairs = queries::track::track_ids_by_albums(&db, &[album_id(&db, "Boxed").await?]).await?;

    assert_eq!(titles_of(&db, &track_ids(&pairs)).await?, ["Opener", "Later", "Second Disc"]);
    Ok(())
}

/// `SQLite` sorts `NULL` first and `ui::track_sort` sorts an untagged track last, and `0` means
/// untagged on both sides. The `COALESCE(NULLIF(...), i32::MAX)` pair is the whole of what closes
/// that; without it this fixture comes back in exactly the order it was inserted.
#[tokio::test]
async fn an_untagged_track_number_sorts_last_rather_than_first() -> Result<(), AppError> {
    let db = empty_library().await?;
    stage(&db, &numbered("Null Number", "Mixed", Some(1), None)).await?;
    stage(&db, &numbered("Zero Number", "Mixed", Some(1), Some(0))).await?;
    stage(&db, &numbered("Numbered", "Mixed", Some(1), Some(1))).await?;

    let pairs = queries::track::track_ids_by_albums(&db, &[album_id(&db, "Mixed").await?]).await?;

    // The two untagged ones tie, and the title breaks it.
    assert_eq!(
        titles_of(&db, &track_ids(&pairs)).await?,
        ["Numbered", "Null Number", "Zero Number"]
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_disc_number_reads_as_disc_one() -> Result<(), AppError> {
    let db = empty_library().await?;
    stage(&db, &numbered("No Disc", "Discs", None, Some(5))).await?;
    stage(&db, &numbered("Disc Zero", "Discs", Some(0), Some(3))).await?;
    stage(&db, &numbered("Disc One", "Discs", Some(1), Some(1))).await?;
    stage(&db, &numbered("Disc Two", "Discs", Some(2), Some(1))).await?;

    let pairs = queries::track::track_ids_by_albums(&db, &[album_id(&db, "Discs").await?]).await?;

    assert_eq!(
        titles_of(&db, &track_ids(&pairs)).await?,
        ["Disc One", "Disc Zero", "No Disc", "Disc Two"]
    );
    Ok(())
}

/// Artist Detail defaults to `"album"`, so the card beside it has to queue by album and then
/// title. The fixture's title order is deliberately the opposite, which is what `sort_key` (the
/// `for_list` sibling's clause) would have answered.
#[tokio::test]
async fn artist_tracks_come_back_by_album_then_title() -> Result<(), AppError> {
    let db = empty_library().await?;
    let solo = "Solo Artist";
    for (title, album) in [("Alpha", "Zulu"), ("Zulu", "Alpha"), ("Beta", "Alpha")] {
        let mut meta = numbered(title, album, Some(1), Some(1));
        meta.artist = ArtistCredit::from_name(solo);
        stage(&db, &meta).await?;
    }

    let pairs = queries::track::track_ids_by_artists(&db, &[artist_id(&db, solo).await?]).await?;

    assert_eq!(titles_of(&db, &track_ids(&pairs)).await?, ["Beta", "Zulu", "Alpha"]);
    Ok(())
}

#[tokio::test]
async fn genre_tracks_come_back_by_album_then_title() -> Result<(), AppError> {
    let db = empty_library().await?;
    let only = "Shoegaze";
    for (title, album) in [("Alpha", "Zulu"), ("Zulu", "Alpha"), ("Beta", "Alpha")] {
        let mut meta = numbered(title, album, Some(1), Some(1));
        meta.genres = GenreList::from_name(only);
        stage(&db, &meta).await?;
    }

    let pairs = queries::track::track_ids_by_genres(&db, &[genre_id(&db, only).await?]).await?;

    assert_eq!(titles_of(&db, &track_ids(&pairs)).await?, ["Beta", "Zulu", "Alpha"]);
    Ok(())
}

/// The comparator reads a missing album as `""`, so an untagged release belongs beside an empty
/// one rather than ahead of everything. A bare `t.album ASC` puts the `NULL` first.
#[tokio::test]
async fn a_missing_album_sorts_beside_an_empty_one() -> Result<(), AppError> {
    let db = empty_library().await?;
    let solo = "Untagged Artist";
    let staged: [(&str, Option<&str>); 3] =
        [("Beta", Some("Mid")), ("Zulu", None), ("Alpha", Some(""))];
    for (title, album) in staged {
        let mut meta = numbered(title, "", Some(1), Some(1));
        meta.album = album.map(str::to_owned);
        meta.artist = ArtistCredit::from_name(solo);
        stage(&db, &meta).await?;
    }

    let pairs = queries::track::track_ids_by_artists(&db, &[artist_id(&db, solo).await?]).await?;

    assert_eq!(titles_of(&db, &track_ids(&pairs)).await?, ["Alpha", "Zulu", "Beta"]);
    Ok(())
}

/// The join's documented consequence, and why the caller's dedupe is load-bearing rather than
/// tidy: one track, two selected artists, two pairs.
#[tokio::test]
async fn a_track_credited_to_two_selected_artists_is_returned_for_each() -> Result<(), AppError> {
    let db = empty_library().await?;
    let mut meta = numbered("Duet", "Together", Some(1), Some(1));
    meta.artist =
        ArtistCredit::from_tags("First & Second", &["First".to_owned(), "Second".to_owned()]);
    let track = stage(&db, &meta).await?;

    let first = artist_id(&db, "First").await?;
    let second = artist_id(&db, "Second").await?;
    let pairs = queries::track::track_ids_by_artists(&db, &[first, second]).await?;

    let mut answered = pairs.clone();
    answered.sort_unstable();
    assert_eq!(answered, [(first, track), (second, track)]);
    Ok(())
}

#[tokio::test]
async fn playlist_tracks_come_back_in_playlist_position() -> Result<(), AppError> {
    let db = empty_library().await?;
    let first = stage(&db, &numbered("Alpha", "Ordered", Some(1), Some(1))).await?;
    let second = stage(&db, &numbered("Beta", "Ordered", Some(1), Some(2))).await?;
    let third = stage(&db, &numbered("Gamma", "Ordered", Some(1), Some(3))).await?;

    // Neither rowid order nor any tag order.
    let playlist =
        queries::playlist::create_playlist_with_tracks(&db, "Mine", None, &[third, first, second])
            .await?;

    let pairs = queries::track::track_ids_by_playlists(&db, &[playlist]).await?;

    assert_eq!(track_ids(&pairs), [third, first, second]);
    Ok(())
}

/// A smart playlist keeps no `playlist_items` rows at all, which is why the caller splits the two
/// kinds before asking rather than falling back on an empty answer.
#[tokio::test]
async fn a_smart_playlist_has_no_membership_rows() -> Result<(), AppError> {
    let db = empty_library().await?;
    stage(&db, &numbered("Alpha", "Ordered", Some(1), Some(1))).await?;
    let smart = queries::playlist::create_smart_playlist(&db, "Rules", None, "{}").await?;

    let pairs = queries::track::track_ids_by_playlists(&db, &[smart.id]).await?;

    assert!(pairs.is_empty(), "a smart playlist resolves from its criteria, not from a join table");
    Ok(())
}

/// `chunked_in_query` splits on the *entity* ids, so every one of an entity's rows lands in a
/// single chunk and its `ORDER BY` survives. Nothing else states that, and a split on track ids
/// would interleave two albums at the seam.
#[tokio::test]
async fn every_entity_keeps_its_order_across_a_chunk_boundary() -> Result<(), AppError> {
    let db = empty_library().await?;
    // Staged out of track order, so a chunk answered by a table scan comes back wrong.
    let staged =
        [("Early", [("E3", 3), ("E1", 1), ("E2", 2)]), ("Late", [("L3", 3), ("L1", 1), ("L2", 2)])];
    for (album, tracks) in staged {
        for (title, number) in tracks {
            stage(&db, &numbered(title, album, Some(1), Some(number))).await?;
        }
    }
    let early = album_id(&db, "Early").await?;
    let late = album_id(&db, "Late").await?;

    // Ids no album carries, so each chunk holds exactly one real album and the seam falls between
    // them. `MAX_BINDS_PER_STATEMENT` is the split.
    let mut asked = vec![early];
    asked.extend(
        (0..crate::database::MAX_BINDS_PER_STATEMENT - 1)
            .map(|offset| ABSENT_ENTITY_ID + i64::try_from(offset).unwrap_or(0)),
    );
    asked.push(late);

    let pairs = queries::track::track_ids_by_albums(&db, &asked).await?;

    let early_tracks: Vec<i64> =
        pairs.iter().filter(|&&(id, _)| id == early).map(|&(_, t)| t).collect();
    let late_tracks: Vec<i64> =
        pairs.iter().filter(|&&(id, _)| id == late).map(|&(_, t)| t).collect();
    assert_eq!(titles_of(&db, &early_tracks).await?, ["E1", "E2", "E3"]);
    assert_eq!(titles_of(&db, &late_tracks).await?, ["L1", "L2", "L3"]);
    Ok(())
}

#[tokio::test]
async fn an_empty_id_list_asks_the_database_nothing() -> Result<(), AppError> {
    let db = empty_library().await?;
    assert!(queries::track::track_ids_by_albums(&db, &[]).await?.is_empty());
    assert!(queries::track::track_ids_by_artists(&db, &[]).await?.is_empty());
    assert!(queries::track::track_ids_by_genres(&db, &[]).await?.is_empty());
    assert!(queries::track::track_ids_by_playlists(&db, &[]).await?.is_empty());
    Ok(())
}

/// "Play this folder" means the subtree, where Browse *lists* one level. An artist folder holding
/// only album subfolders would otherwise play nothing at all.
#[tokio::test]
async fn a_folder_card_plays_its_subfolders_too() -> Result<(), AppError> {
    let db = empty_library().await?;
    let artist_dir = Path::new(LIBRARY_ROOT).join("Artist");
    let album_dir = artist_dir.join("Album");

    for (dir, title) in [(&artist_dir, "Loose"), (&album_dir, "Nested")] {
        let meta = numbered(title, "Whatever", Some(1), Some(1));
        insert_tagged_track(&db, &dir.join(format!("{title}.mp3")).to_string_lossy(), &meta)
            .await?;
    }

    let ids = queries::track::track_ids_under_directory(&db, &artist_dir.to_string_lossy()).await?;

    // Path order, and the nested folder sorts ahead of the loose file on either separator.
    assert_eq!(titles_of(&db, &ids).await?, ["Nested", "Loose"]);
    Ok(())
}

/// `library::browse` lists a directory's files through `to_lowercase()`, so under the default
/// BINARY collation the queue and the page it was raised from disagree from the first row.
#[tokio::test]
async fn a_folder_queues_in_the_order_browse_lists_it() -> Result<(), AppError> {
    let db = empty_library().await?;
    let dir = Path::new(LIBRARY_ROOT).join("Mixed Case");

    for title in ["Zebra", "apple"] {
        let meta = numbered(title, "Whatever", Some(1), Some(1));
        insert_tagged_track(&db, &dir.join(format!("{title}.mp3")).to_string_lossy(), &meta)
            .await?;
    }

    let ids = queries::track::track_ids_under_directory(&db, &dir.to_string_lossy()).await?;

    assert_eq!(titles_of(&db, &ids).await?, ["apple", "Zebra"]);
    Ok(())
}
