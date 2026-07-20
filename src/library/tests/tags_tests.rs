//! Orchestrator tests — target the testable `write_tag_edit` core (no
//! `AppState`, no player) against a `test_pool` and real fixtures copied out of
//! `tests/assets/` into a `TempDir`. Never write to the checked-in asset.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

use super::write_tag_edit;
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::insert_test_track;
use crate::error::AppError;
use crate::media::artwork;
use crate::media::self_writes::SelfWrites;
use crate::media::tag_writer::{FieldEdit, TagEdit};

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

/// Copy a checked-in fixture into `tmp` and hand back the working copy.
fn stage(tmp: &TempDir, name: &str) -> Result<PathBuf, AppError> {
    let src = assets_dir().join(name);
    let dst = tmp.path().join(name);
    std::fs::copy(&src, &dst)?;
    Ok(dst)
}

async fn seed_track(db: &DbPool, path: &str) -> Result<i64, AppError> {
    insert_test_track(db, path, "Old Title", "Old Artist", "Old Album", "Rock").await
}

#[tokio::test]
async fn single_track_edit_updates_row_and_preserves_stats() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let path = stage(&tmp, "silence.mp3")?;
    let path_str = path.to_string_lossy().into_owned();
    let id = seed_track(&db, &path_str).await?;

    // Playback state that a metadata refresh must preserve.
    sqlx::query("UPDATE tracks SET play_count = 5, rating = 4, is_favorite = 1 WHERE id = ?")
        .bind(id)
        .execute(db.write())
        .await?;
    let old_hash: String = sqlx::query_scalar("SELECT file_hash FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;

    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let cover_cache = artwork::new_cover_cache();
    let self_writes = Arc::new(SelfWrites::default());

    let edit = TagEdit {
        album: FieldEdit::Set("Brand New Album".to_owned()),
        ..TagEdit::default()
    };
    let (report, updated) =
        write_tag_edit(&db, &artwork_dir, &cover_cache, &self_writes, &[id], &edit, None).await?;

    assert_eq!(report.updated, 1);
    assert_eq!(updated, vec![id]);
    assert!(report.failures.is_empty());

    let (album, play_count, rating, is_favorite, new_hash): (String, i64, i64, i64, String) =
        sqlx::query_as(
            "SELECT album, play_count, rating, is_favorite, file_hash FROM tracks WHERE id = ?",
        )
        .bind(id)
        .fetch_one(db.read())
        .await?;

    assert_eq!(album, "Brand New Album");
    assert_eq!(play_count, 5, "play_count must survive the metadata refresh");
    assert_eq!(rating, 4, "rating must survive the metadata refresh");
    assert_eq!(is_favorite, 1, "is_favorite must survive the metadata refresh");
    assert_ne!(
        new_hash, old_hash,
        "a tag write rewrites the file, so file_hash changes"
    );
    Ok(())
}

#[tokio::test]
async fn batch_edit_reports_failure_and_commits_the_rest() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let good = stage(&tmp, "silence.mp3")?;
    let good_str = good.to_string_lossy().into_owned();
    let good_id = seed_track(&db, &good_str).await?;

    // A DB row whose file does not exist on disk — the write fails at read.
    let ghost_str = tmp
        .path()
        .join("ghost.mp3")
        .to_string_lossy()
        .into_owned();
    let ghost_id = seed_track(&db, &ghost_str).await?;

    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let cover_cache = artwork::new_cover_cache();
    let self_writes = Arc::new(SelfWrites::default());

    let edit = TagEdit {
        title: FieldEdit::Set("Renamed".to_owned()),
        ..TagEdit::default()
    };
    let (report, updated) = write_tag_edit(
        &db,
        &artwork_dir,
        &cover_cache,
        &self_writes,
        &[good_id, ghost_id],
        &edit,
        None,
    )
    .await?;

    assert_eq!(report.updated, 1);
    assert_eq!(updated, vec![good_id]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].0, ghost_str);

    let title: String = sqlx::query_scalar("SELECT title FROM tracks WHERE id = ?")
        .bind(good_id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(title, "Renamed", "the good row still committed its refresh");
    Ok(())
}

#[tokio::test]
async fn album_rename_moves_track_to_new_album_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let path = stage(&tmp, "silence.flac")?;
    let path_str = path.to_string_lossy().into_owned();
    let id = seed_track(&db, &path_str).await?;

    let old_album_id: i64 = sqlx::query_scalar("SELECT album_id FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;

    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let cover_cache = artwork::new_cover_cache();
    let self_writes = Arc::new(SelfWrites::default());

    let edit = TagEdit {
        album: FieldEdit::Set("A Totally Different Album".to_owned()),
        ..TagEdit::default()
    };
    write_tag_edit(&db, &artwork_dir, &cover_cache, &self_writes, &[id], &edit, None).await?;

    let new_album_id: i64 = sqlx::query_scalar("SELECT album_id FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;
    assert_ne!(new_album_id, old_album_id, "the album rename must repoint album_id");

    let name: String = sqlx::query_scalar("SELECT name FROM albums WHERE id = ?")
        .bind(new_album_id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(name, "A Totally Different Album");
    Ok(())
}
