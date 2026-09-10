//! What the pass answers its caller with.
//!
//! The bounds themselves are `library::lyrics`' own suite's. What only this level can see is that
//! the count comes back rather than being swallowed, and that a store the pass cannot read is a
//! reported failure rather than a silent zero: a cache that has stopped pruning looks exactly like
//! one with nothing to prune.

use std::sync::Arc;

use tempfile::TempDir;

use super::*;
use melodia_core::config::Paths;

/// A data root with the lyrics directory already made, which is what a real boot hands over.
fn store() -> Result<(TempDir, Arc<Paths>), AppError> {
    let tmp = TempDir::new()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    Ok((tmp, Arc::new(paths)))
}

#[tokio::test]
async fn an_empty_store_retires_nothing() -> Result<(), AppError> {
    let (_tmp, paths) = store()?;

    assert_eq!(run(paths).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_store_inside_its_bounds_retires_nothing() -> Result<(), AppError> {
    // A sheet fetched a moment ago is the ordinary case, and a pass that retired it would cost a
    // request per track change for as long as the view stayed open.
    let (_tmp, paths) = store()?;
    std::fs::write(paths.lyrics_dir.join("0123456789abcdef.lrc"), "[00:01.00]words")?;

    assert_eq!(run(Arc::clone(&paths)).await?, 0);
    assert!(paths.lyrics_dir.join("0123456789abcdef.lrc").is_file());
    Ok(())
}

#[tokio::test]
async fn a_store_that_cannot_be_read_is_reported_rather_than_counted_as_empty()
-> Result<(), AppError> {
    let (_tmp, paths) = store()?;
    std::fs::remove_dir_all(&paths.lyrics_dir)?;

    assert!(run(paths).await.is_err());
    Ok(())
}
