//! Backfill-writer tests — target the testable `write_mbids` core (no
//! `AppState`) against a `test_pool` and real fixtures copied out of
//! `tests/assets/` into a `TempDir`. Never write to the checked-in asset.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

use super::{ResolvedMbid, write_mbids};
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::insert_test_track;
use crate::error::AppError;
use crate::media::artwork;
use crate::media::self_writes::SelfWrites;
use crate::test_support::ASSETS_DIR;

fn assets_dir() -> PathBuf {
    PathBuf::from(ASSETS_DIR)
}

fn stage(tmp: &TempDir, name: &str) -> Result<PathBuf, AppError> {
    let src = assets_dir().join(name);
    let dst = tmp.path().join(name);
    std::fs::copy(&src, &dst)?;
    Ok(dst)
}

async fn seed_track(db: &DbPool, path: &str) -> Result<i64, AppError> {
    insert_test_track(db, path, "Candy Shop", "50 Cent", "The Massacre", "Hip-Hop").await
}

#[tokio::test]
async fn writes_recording_id_to_file_and_db_and_preserves_stats() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let path = stage(&tmp, "silence.mp3")?;
    let path_str = path.to_string_lossy().into_owned();
    let id = seed_track(&db, &path_str).await?;

    sqlx::query("UPDATE tracks SET play_count = 7, rating = 5, is_favorite = 1 WHERE id = ?")
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

    let mbid = "189002e7-3285-4e2e-92a3-7f6c30d407a2";
    let resolved: Vec<ResolvedMbid> = vec![(id, path_str.clone(), mbid.to_owned())];
    let updated = write_mbids(&db, &artwork_dir, &cover_cache, &self_writes, &resolved).await?;

    assert_eq!(updated, 1);

    let (stored_mbid, play_count, rating, is_favorite, new_hash): (
        Option<String>,
        i64,
        i64,
        i64,
        String,
    ) = sqlx::query_as(
        "SELECT musicbrainz_track_id, play_count, rating, is_favorite, file_hash
         FROM tracks WHERE id = ?",
    )
    .bind(id)
    .fetch_one(db.read())
    .await?;

    assert_eq!(stored_mbid.as_deref(), Some(mbid));
    assert_eq!(play_count, 7, "play_count must survive the MBID refresh");
    assert_eq!(rating, 5, "rating must survive the MBID refresh");
    assert_eq!(is_favorite, 1, "is_favorite must survive the MBID refresh");
    assert_ne!(new_hash, old_hash, "writing the tag rewrites the file");

    // The write was marked so the watcher ignores its own echo.
    assert!(self_writes.take_recent(&path), "the written path must be marked in SelfWrites");
    Ok(())
}

#[tokio::test]
async fn a_missing_file_is_skipped_and_the_rest_commit() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let good = stage(&tmp, "silence.mp3")?;
    let good_str = good.to_string_lossy().into_owned();
    let good_id = seed_track(&db, &good_str).await?;

    let ghost_str = tmp.path().join("ghost.mp3").to_string_lossy().into_owned();
    let ghost_id = seed_track(&db, &ghost_str).await?;

    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let cover_cache = artwork::new_cover_cache();
    let self_writes = Arc::new(SelfWrites::default());

    let resolved: Vec<ResolvedMbid> = vec![
        (good_id, good_str, "rec-good".to_owned()),
        (ghost_id, ghost_str, "rec-ghost".to_owned()),
    ];
    let updated = write_mbids(&db, &artwork_dir, &cover_cache, &self_writes, &resolved).await?;

    // Only the file that exists is written; the ghost is logged and skipped.
    assert_eq!(updated, 1);
    let good_mbid: Option<String> =
        sqlx::query_scalar("SELECT musicbrainz_track_id FROM tracks WHERE id = ?")
            .bind(good_id)
            .fetch_one(db.read())
            .await?;
    assert_eq!(good_mbid.as_deref(), Some("rec-good"));
    let ghost_mbid: Option<String> =
        sqlx::query_scalar("SELECT musicbrainz_track_id FROM tracks WHERE id = ?")
            .bind(ghost_id)
            .fetch_one(db.read())
            .await?;
    assert!(ghost_mbid.is_none(), "the missing file's row is untouched");
    Ok(())
}
