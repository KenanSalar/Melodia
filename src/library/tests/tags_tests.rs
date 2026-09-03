//! Orchestrator tests — target the testable `write_tag_edit` core (no
//! `AppState`, no player) against a `test_pool` and real fixtures copied out of
//! `tests/assets/` into a `TempDir`. Never write to the checked-in asset.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use super::write_tag_edit;
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::insert_test_track;
use crate::entities::tags::{ArtworkEdit, FieldEdit, TagEdit};
use crate::error::AppError;
use crate::media::image::artwork;
use crate::test_support::ASSETS_DIR;
use crate::utils::self_writes::SelfWrites;

fn assets_dir() -> PathBuf {
    PathBuf::from(ASSETS_DIR)
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
    let db = DbPool::test_pool().await?;
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
    assert_ne!(new_hash, old_hash, "a tag write rewrites the file, so file_hash changes");
    Ok(())
}

#[tokio::test]
async fn batch_edit_reports_failure_and_commits_the_rest() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let good = stage(&tmp, "silence.mp3")?;
    let good_str = good.to_string_lossy().into_owned();
    let good_id = seed_track(&db, &good_str).await?;

    // A DB row whose file does not exist on disk — the write fails at read.
    let ghost_str = tmp.path().join("ghost.mp3").to_string_lossy().into_owned();
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
    let db = DbPool::test_pool().await?;
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

/// Seed the file's own genre so a later re-extract has nothing to disagree with. Without it the
/// fixture's tags decide where the track lands and "the parent did not move" proves nothing.
async fn set_genre(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &artwork::CoverCache,
    self_writes: &Arc<SelfWrites>,
    id: i64,
    genre: &str,
) -> Result<(String, i64), AppError> {
    let edit = TagEdit {
        genre: FieldEdit::Set(genre.to_owned()),
        ..TagEdit::default()
    };
    write_tag_edit(db, artwork_dir, cover_cache, self_writes, &[id], &edit, None).await?;

    let row: (String, i64) = sqlx::query_as("SELECT genre, genre_id FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(row.0, genre, "the genre write has to land, or the caller is testing nothing");
    Ok(row)
}

/// A rating write reaches `run_commit` on a single click, so both whole-table passes are gated
/// off it. What that gate has to be worth: an orphan left by something else survives, and the
/// re-extract it also skips does not take the cover with it.
#[tokio::test]
async fn a_rating_only_edit_sweeps_nothing_and_keeps_its_cover() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let path = stage(&tmp, "silence.flac")?;
    let id = seed_track(&db, &path.to_string_lossy()).await?;

    // An external cover the re-extract *would* find, so the artwork assertion below fails when
    // the skip stops happening rather than passing because there was no art to find either way.
    // Ahead of every write: `find_external_cover` memoizes per directory on the first pass, so a
    // cover dropped in later is one the cache has already answered `None` for.
    std::fs::copy(assets_dir().join("cover.jpg"), tmp.path().join("cover.jpg"))?;

    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let cover_cache = artwork::new_cover_cache();
    let self_writes = Arc::new(SelfWrites::default());

    let (_, genre_before) =
        set_genre(&db, &artwork_dir, &cover_cache, &self_writes, id, "Kept Genre").await?;

    // A cover on the row and an orphan beside it: the two things the gated passes would reach.
    sqlx::query("UPDATE tracks SET artwork_path = ? WHERE id = ?")
        .bind("/covers/kept.jpg")
        .bind(id)
        .execute(db.write())
        .await?;
    sqlx::query("INSERT INTO genres (name) VALUES ('Stray Genre')").execute(db.write()).await?;

    let edit = TagEdit {
        rating: FieldEdit::Set(4),
        ..TagEdit::default()
    };
    let (report, _) =
        write_tag_edit(&db, &artwork_dir, &cover_cache, &self_writes, &[id], &edit, None).await?;
    assert_eq!(report.updated, 1);
    assert!(report.failures.is_empty());

    let (artwork_path, genre_after, rating): (String, i64, i64) =
        sqlx::query_as("SELECT artwork_path, genre_id, rating FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_one(db.read())
            .await?;

    assert_eq!(rating, 4, "the star still reaches the row through the re-extract");
    assert_eq!(artwork_path, "/covers/kept.jpg", "the skipped re-extract must not null the cover");
    assert_eq!(genre_after, genre_before, "a rating write re-homes nothing");

    let strays: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM genres WHERE name = 'Stray Genre'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(strays, 1, "a rating write must not pay for a whole-table orphan sweep");
    Ok(())
}

/// The other half of the gate: an edit that really does re-home the track still sweeps the parent
/// it emptied. A field dropping out of `moves_between_parents` leaves orphans and nothing else
/// notices, `prune_orphans` being the only thing in the tree that deletes an emptied row.
#[tokio::test]
async fn a_genre_edit_still_sweeps_the_genre_it_emptied() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let path = stage(&tmp, "silence.flac")?;
    let id = seed_track(&db, &path.to_string_lossy()).await?;

    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let cover_cache = artwork::new_cover_cache();
    let self_writes = Arc::new(SelfWrites::default());

    let (_, emptied_id) =
        set_genre(&db, &artwork_dir, &cover_cache, &self_writes, id, "Genre Before").await?;
    let (_, landed_id) =
        set_genre(&db, &artwork_dir, &cover_cache, &self_writes, id, "Genre After").await?;

    assert_ne!(landed_id, emptied_id, "the genre rename must repoint genre_id");

    let survivors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM genres WHERE id = ?")
        .bind(emptied_id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(survivors, 0, "the genre the move emptied is swept in the same transaction");
    Ok(())
}

/// A cover `Replace` across a same-album batch: every track row **and** the
/// shared album row must end on the cached picked cover, and the memoized FK
/// resolve must hand both tracks one `album_id`. Guards the `skip_artwork` branch
/// (the re-extract's per-track cover work is discarded here) and
/// `apply_replace_artwork`.
#[tokio::test]
async fn replace_artwork_lands_on_every_track_and_the_shared_album() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let tmp = TempDir::new()?;
    let folder = tmp.path().to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

    let a = stage(&tmp, "silence.flac")?;
    let b = stage(&tmp, "silence.mp3")?;
    let a_id = seed_track(&db, &a.to_string_lossy()).await?;
    let b_id = seed_track(&db, &b.to_string_lossy()).await?;

    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let cover_cache = artwork::new_cover_cache();
    let self_writes = Arc::new(SelfWrites::default());

    // Write a deterministic shared artist/album so the re-extract lands both
    // files in one album regardless of the fixtures' own tags — then the memo
    // and the Replace roll-up have a single album to converge on.
    let edit = TagEdit {
        artist: FieldEdit::Set("Shared Artist".to_owned()),
        album: FieldEdit::Set("Shared Album".to_owned()),
        artwork: ArtworkEdit::Replace,
        ..TagEdit::default()
    };
    let source = assets_dir().join("cover.jpg");
    let (report, updated) = write_tag_edit(
        &db,
        &artwork_dir,
        &cover_cache,
        &self_writes,
        &[a_id, b_id],
        &edit,
        Some(source.as_path()),
    )
    .await?;

    assert_eq!(report.updated, 2);
    assert_eq!(updated, vec![a_id, b_id]);
    assert!(report.failures.is_empty());

    // Bind to non-null types (both columns are set after a successful Replace),
    // so a NULL would fail the test rather than needing an `expect`.
    let (art_a, album_a): (String, i64) =
        sqlx::query_as("SELECT artwork_path, album_id FROM tracks WHERE id = ?")
            .bind(a_id)
            .fetch_one(db.read())
            .await?;
    let (art_b, album_b): (String, i64) =
        sqlx::query_as("SELECT artwork_path, album_id FROM tracks WHERE id = ?")
            .bind(b_id)
            .fetch_one(db.read())
            .await?;

    assert_eq!(art_a, art_b, "both tracks share the replaced cover");
    assert!(
        art_a.starts_with(&*artwork_dir.to_string_lossy()),
        "the cover is cached under the artwork dir"
    );
    assert_eq!(album_a, album_b, "same-album batch resolves to one album_id");

    let album_art: String = sqlx::query_scalar("SELECT artwork_path FROM albums WHERE id = ?")
        .bind(album_a)
        .fetch_one(db.read())
        .await?;
    assert_eq!(album_art, art_a, "the album card gets the replaced cover too");

    Ok(())
}
