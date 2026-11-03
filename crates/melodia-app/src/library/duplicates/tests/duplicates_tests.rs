//! A find over a real library, and Not Duplicates' memory across finds. The grouping rule itself
//! is `matcher_tests`'; these hold the two reads and the dismissal sets they carry.

use super::*;
use melodia_store::database::queries::fixtures::insert_test_track;

/// A library folder and two copies of "Song", identical files since the fixture hashes the title.
async fn two_copies() -> Result<(DbPool, i64, i64), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let first = song_at(&db, "/music/a/song.mp3").await?;
    let second = song_at(&db, "/music/b/song.mp3").await?;
    Ok((db, first, second))
}

async fn song_at(db: &DbPool, path: &str) -> Result<i64, AppError> {
    insert_test_track(db, path, "Song", "Artist", "Album", "Rock").await
}

fn ids(groups: &[DuplicateGroup]) -> Vec<Vec<i64>> {
    groups.iter().map(|group| group.copies.iter().map(|copy| copy.id).collect()).collect()
}

fn dismissals(groups: &[DuplicateGroup]) -> Vec<Option<DismissalSet>> {
    groups.iter().map(|group| group.dismissed).collect()
}

/// Two rows of unknown length sharing their tags say nothing about being one song.
#[tokio::test]
async fn a_find_groups_the_copies_of_one_song_and_never_rows_of_unknown_length()
-> Result<(), AppError> {
    let (db, first, second) = two_copies().await?;
    for path in ["/music/a/untitled.mp3", "/music/b/untitled.mp3"] {
        insert_test_track(&db, path, "Untitled", "Artist", "Album", "Rock").await?;
    }
    sqlx::query("UPDATE tracks SET duration_ms = 0 WHERE title = 'Untitled'")
        .execute(db.write())
        .await?;

    let groups = find(&db).await?;

    assert_eq!(ids(&groups), [[first, second]]);
    Ok(())
}

#[tokio::test]
async fn not_duplicates_flags_its_group_on_the_next_find() -> Result<(), AppError> {
    let (db, first, second) = two_copies().await?;
    let set = dismiss(&db, &[first, second]).await?;

    let groups = find(&db).await?;

    assert_eq!(dismissals(&groups), [Some(set)]);
    Ok(())
}

#[tokio::test]
async fn a_new_copy_joining_a_dismissed_group_shows_it_again() -> Result<(), AppError> {
    let (db, first, second) = two_copies().await?;
    dismiss(&db, &[first, second]).await?;
    song_at(&db, "/music/c/song.mp3").await?;

    let groups = find(&db).await?;

    assert_eq!(dismissals(&groups), [None]);
    Ok(())
}

/// A copy dismissed again within another group moves to that group's set, leaving its first group
/// split across two.
#[tokio::test]
async fn a_group_whose_copies_sit_in_two_sets_is_not_dismissed() -> Result<(), AppError> {
    let (db, first, second) = two_copies().await?;
    dismiss(&db, &[first, second]).await?;
    dismiss(&db, &[second]).await?;

    let groups = find(&db).await?;

    assert_eq!(dismissals(&groups), [None]);
    Ok(())
}

#[tokio::test]
async fn dismissing_a_grown_group_again_hides_it_whole() -> Result<(), AppError> {
    let (db, first, second) = two_copies().await?;
    dismiss(&db, &[first, second]).await?;
    let third = song_at(&db, "/music/c/song.mp3").await?;
    let set = dismiss(&db, &[first, second, third]).await?;

    let groups = find(&db).await?;

    assert_eq!(dismissals(&groups), [Some(set)]);
    Ok(())
}

#[tokio::test]
async fn count_as_duplicates_unflags_the_group() -> Result<(), AppError> {
    let (db, first, second) = two_copies().await?;
    let set = dismiss(&db, &[first, second]).await?;
    restore(&db, set).await?;

    let groups = find(&db).await?;

    assert_eq!(dismissals(&groups), [None]);
    Ok(())
}
