//! What adding a folder does to the rows already there: nothing, whatever it covers.
//!
//! A child row deleted here would take its tracks with it, `tracks.folder_id` carrying
//! `ON DELETE CASCADE`, and with them the ratings, play counts and favourites no rescan brings
//! back. The parent's first completed scan absorbs the children instead, so an add the user
//! cancels leaves them as they were.

use std::path::Path;

use tempfile::TempDir;

use super::insert_validated;
use melodia_core::error::AppError;
use melodia_store::database::queries::fixtures::insert_test_track;
use melodia_store::database::{DbPool, queries};

/// A real directory under `root`, since validation stats the path before it compares anything.
fn dir(root: &TempDir, name: &str) -> Result<std::path::PathBuf, AppError> {
    let path = root.path().join("music").join(name);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn as_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

async fn track_count(db: &DbPool) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?)
}

#[tokio::test]
async fn a_parent_leaves_the_children_it_covers_and_their_tracks_in_place() -> Result<(), AppError>
{
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let first = dir(&tmp, "first")?;
    let second = dir(&tmp, "second")?;
    queries::folder::insert_folder(&db, &as_str(&first), true).await?;
    queries::folder::insert_folder(&db, &as_str(&second), true).await?;
    insert_test_track(&db, &as_str(&first.join("song.mp3")), "Song", "Artist", "Album", "Rock")
        .await?;

    insert_validated(&db, &as_str(&tmp.path().join("music"))).await?;

    assert_eq!(queries::folder::get_all_folders(&db).await?.len(), 3);
    assert_eq!(track_count(&db).await?, 1, "the child's track waits for the parent's scan");
    Ok(())
}

#[tokio::test]
async fn a_path_already_covered_is_refused_and_deletes_nothing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let parent = dir(&tmp, "parent")?;
    let child = dir(&tmp, "parent/child")?;
    queries::folder::insert_folder(&db, &as_str(&parent), true).await?;
    insert_test_track(&db, &as_str(&child.join("song.mp3")), "Song", "Artist", "Album", "Rock")
        .await?;

    let refused = insert_validated(&db, &as_str(&child)).await;

    assert!(matches!(refused, Err(AppError::Validation(_))));
    assert_eq!(queries::folder::get_all_folders(&db).await?.len(), 1);
    assert_eq!(track_count(&db).await?, 1, "and nothing under the folder it kept was spent");
    Ok(())
}

#[tokio::test]
async fn an_unrelated_path_is_added_beside_what_is_there() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let existing = dir(&tmp, "existing")?;
    let fresh = dir(&tmp, "fresh")?;
    queries::folder::insert_folder(&db, &as_str(&existing), true).await?;

    insert_validated(&db, &as_str(&fresh)).await?;

    assert_eq!(queries::folder::get_all_folders(&db).await?.len(), 2);
    Ok(())
}
