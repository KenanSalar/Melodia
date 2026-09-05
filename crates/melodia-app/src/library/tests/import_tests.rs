//! Tests for the drop and open-with path, against copies of `test-assets/silence.mp3` staged
//! into a `TempDir`.
//!
//! What is worth pinning here is not the ingest underneath, which the scan suites already cover,
//! but the two decisions this module makes over it: which of a dropped batch is refused before
//! the scan runs at all, and the existing-versus-new split that decides whether a file the user
//! drops twice is imported twice.

use tempfile::TempDir;

use super::*;
use melodia_artwork::media::image::artwork;
use melodia_core::error::AppError;
use melodia_testkit::ASSETS_DIR;

/// A staging root plus the artwork directory the scan writes any embedded cover into.
fn staging() -> Result<(TempDir, PathBuf), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    Ok((tmp, artwork_dir))
}

/// Stage the silence fixture under `name` and spell its path the way a drop would.
fn drop_file(dir: &Path, name: &str) -> Result<String, AppError> {
    let dest = dir.join(name);
    std::fs::copy(PathBuf::from(ASSETS_DIR).join("silence.mp3"), &dest)?;
    Ok(dest.to_string_lossy().into_owned())
}

/// A drop is whatever the file manager handed over, so one entry it cannot use must not cost the
/// readable ones beside it. Both refusal arms are here: a wrong extension never reaches the
/// filesystem, and a right extension that does not resolve is refused by the canonicalize.
#[tokio::test]
async fn a_drop_keeps_what_it_can_read_and_names_the_rest() -> Result<(), AppError> {
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;

    let song = drop_file(tmp.path(), "song.mp3")?;
    let notes = tmp.path().join("notes.txt");
    std::fs::write(&notes, b"not audio")?;
    let never_written = tmp.path().join("gone.mp3");

    let batch = vec![
        song,
        notes.to_string_lossy().into_owned(),
        never_written.to_string_lossy().into_owned(),
    ];
    let result = import_files(&db, &artwork_dir, &artwork::new_cover_cache(), &batch).await?;

    assert_eq!(result.imported_count, 1, "the readable file must survive its neighbours");
    assert_eq!(result.track_ids.len(), 1);
    assert_eq!(result.failed_paths.len(), 2, "both refusals owe the user a named path");
    Ok(())
}

/// The other end of that partition: a batch with nothing usable in it still answers, rather than
/// erroring, because the caller reports the failures as a toast.
#[tokio::test]
async fn a_drop_with_nothing_importable_reports_every_path_and_imports_none() -> Result<(), AppError>
{
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;

    let notes = tmp.path().join("notes.txt");
    std::fs::write(&notes, b"not audio")?;
    let batch = vec![
        notes.to_string_lossy().into_owned(),
        "/nowhere/at/all.mp3".to_owned(),
    ];

    let result = import_files(&db, &artwork_dir, &artwork::new_cover_cache(), &batch).await?;

    assert!(result.track_ids.is_empty());
    assert_eq!(result.imported_count, 0);
    assert_eq!(result.failed_paths.len(), 2);
    Ok(())
}

/// The existing-versus-new split is the shape of the whole function. Dropping a file the library
/// already holds must hand back the row it already had: a second row would fork the user's play
/// count and rating away from the track they can see.
#[tokio::test]
async fn a_file_already_in_the_library_keeps_its_id_and_is_not_imported_again()
-> Result<(), AppError> {
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;
    let cover_cache = artwork::new_cover_cache();
    let batch = vec![drop_file(tmp.path(), "song.mp3")?];

    let first = import_files(&db, &artwork_dir, &cover_cache, &batch).await?;
    assert_eq!(first.imported_count, 1);

    let second = import_files(&db, &artwork_dir, &cover_cache, &batch).await?;
    assert_eq!(second.imported_count, 0, "the same file is not an import the second time");
    assert_eq!(second.track_ids, first.track_ids, "and comes back as the row it already was");
    Ok(())
}

/// `summaries` is the only thing separating the two entry points, and the queue's drop path reads
/// an empty one as "nothing playable arrived" — so the ids-only half owes an empty vector rather
/// than paying for a projection nobody asked for.
#[tokio::test]
async fn summaries_arrive_one_per_id_and_the_ids_only_half_fetches_none() -> Result<(), AppError> {
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;
    let cover_cache = artwork::new_cover_cache();
    let batch = vec![
        drop_file(tmp.path(), "a.mp3")?,
        drop_file(tmp.path(), "b.mp3")?,
    ];

    let summarized = import_and_summarize(&db, &artwork_dir, &cover_cache, &batch).await?;
    assert_eq!(summarized.track_ids.len(), 2);
    assert_eq!(summarized.summaries.len(), summarized.track_ids.len());

    let ids_only = import_files(&db, &artwork_dir, &cover_cache, &batch).await?;
    assert!(ids_only.summaries.is_empty());

    let empty = import_and_summarize(&db, &artwork_dir, &cover_cache, &[]).await?;
    assert!(empty.summaries.is_empty(), "an empty batch must not reach the projection query");
    Ok(())
}
