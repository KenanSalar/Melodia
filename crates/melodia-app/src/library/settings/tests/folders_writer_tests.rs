//! What adding a folder does to the rows already there.
//!
//! `validate_folder_path`'s own suite settles which paths are children; nothing until now drove
//! the consumer, and the consumer is where the loss happens. A folder superseded by a new parent
//! is a hard `DELETE`, and `tracks.folder_id` carries `ON DELETE CASCADE` — so the tracks under it
//! go too, and the rescan that brings the files back brings none of the ratings, play counts or
//! favourites that were on them.

use std::path::Path;

use tempfile::TempDir;

use super::insert_replacing_children;
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

/// The one that costs the user something. Both child rows go, and the tracks they held go with
/// them — a version that skipped the delete would leave three overlapping folders scanning the
/// same files, which is visible; this direction is not.
#[tokio::test]
async fn a_parent_replaces_the_children_it_covers_and_their_tracks_go_too() -> Result<(), AppError>
{
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let first = dir(&tmp, "first")?;
    let second = dir(&tmp, "second")?;
    queries::folder::insert_folder(&db, &as_str(&first), true).await?;
    queries::folder::insert_folder(&db, &as_str(&second), true).await?;
    insert_test_track(&db, &as_str(&first.join("song.mp3")), "Song", "Artist", "Album", "Rock")
        .await?;

    let parent = insert_replacing_children(&db, &as_str(&tmp.path().join("music"))).await?;

    let folders = queries::folder::get_all_folders(&db).await?;
    assert_eq!(folders.len(), 1, "the parent is the only folder left");
    assert_eq!(folders.first().map(|f| f.id), Some(parent.id));
    assert_eq!(track_count(&db).await?, 0, "and the cascade took the track with the child row");
    Ok(())
}

/// A path already covered has to be refused *before* the delete, not after it: the ladder deletes
/// what it takes for children, and an error raised later would have already spent them.
#[tokio::test]
async fn a_path_already_covered_is_refused_and_deletes_nothing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let parent = dir(&tmp, "parent")?;
    let child = dir(&tmp, "parent/child")?;
    queries::folder::insert_folder(&db, &as_str(&parent), true).await?;
    insert_test_track(&db, &as_str(&child.join("song.mp3")), "Song", "Artist", "Album", "Rock")
        .await?;

    let refused = insert_replacing_children(&db, &as_str(&child)).await;

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

    insert_replacing_children(&db, &as_str(&fresh)).await?;

    assert_eq!(queries::folder::get_all_folders(&db).await?.len(), 2);
    Ok(())
}
