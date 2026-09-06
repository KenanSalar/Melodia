//! The pass that re-encodes an old store, and the one thing it must never start doing.
//!
//! It runs behind a `one_shot` marker, so whatever it does to an install it does once and never
//! again. Its contract is a negative — the original is left where it is, unreferenced, for the
//! sweep it awaits to retire on a later launch — and a negative is exactly the kind of rule that
//! erodes without anything failing.

use std::path::Path;

use tempfile::TempDir;

use super::*;
use melodia_store::database::queries::fixtures::insert_test_track;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A store rooted in a temp dir, with the folder row a track insert needs under it.
struct Library {
    db: DbPool,
    paths: Paths,
    _tmp: TempDir,
}

impl Library {
    async fn new() -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let paths = Paths::rooted_at(tmp.path().to_path_buf());
        paths.create_dirs()?;
        let db = DbPool::test_pool().await?;
        queries::folder::insert_folder(&db, "/music", true).await?;
        Ok(Self {
            db,
            paths,
            _tmp: tmp,
        })
    }

    /// Point the one track row at `stored`, which is the only thing that puts a file in front of
    /// the pass — it reads `referenced_paths`, never the directory.
    async fn refer_to(&self, stored: &Path) -> Result<(), AppError> {
        let id = insert_test_track(&self.db, "/music/track.mp3", "Song", "Artist", "Album", "Rock")
            .await?;
        let mut tx = self.db.write().begin().await?;
        queries::track::set_track_artwork(&mut tx, &[id], Some(&stored.to_string_lossy())).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn artwork_path(&self) -> Result<Option<String>, AppError> {
        let path = sqlx::query_scalar::<_, Option<String>>("SELECT artwork_path FROM tracks")
            .fetch_one(self.db.read())
            .await?;
        Ok(path)
    }
}

/// Every file the store holds, sorted so the assertion does not ride on readdir order.
fn store_contents(dir: &Path) -> Result<Vec<String>, AppError> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort();
    Ok(names)
}

/// The whole pass in one case, and the negative is the half that matters. A re-encode that also
/// unlinked its source would be correct on the day it was written — the rows are re-pointed in
/// the same transaction — and wrong the moment anything else still names those bytes, with no
/// second launch to notice: the marker is already set.
#[tokio::test]
async fn an_oversized_cover_is_re_pointed_and_its_original_left_where_it_was() -> TestResult {
    let library = Library::new().await?;
    // Past `STORE_MAX_DIM`, and a gradient rather than a fill so the re-encode genuinely comes
    // out smaller and the never-inflate rule does not spare it. Copied in raw rather than stored
    // through `store_image`, which is the point: this is the shape a build with no bounds left
    // behind, and today's writer would have shrunk it on the way in.
    let (_source_dir, source) = melodia_testkit::write_test_jpeg_sized(1024, 1024)?;
    let original = library.paths.artwork_dir.join("33fb807d1f1b7cbb.jpg");
    std::fs::copy(&source, &original)?;
    library.refer_to(&original).await?;

    renormalize(&library.db, &library.paths).await?;

    let repointed = library.artwork_path().await?.ok_or("the row lost its artwork entirely")?;
    assert_ne!(repointed, original.to_string_lossy(), "an oversized cover must be re-encoded");
    assert!(
        std::fs::metadata(&repointed)?.len() < std::fs::metadata(&original)?.len(),
        "a re-point onto a file no smaller than the source is the pass doing nothing loudly"
    );
    assert!(original.exists(), "the original is the sweep's to retire, not this pass's to unlink");
    Ok(())
}

/// The common case, and what stops the case above passing on a pass that rewrites everything it
/// is handed. A store already inside the bounds is the state every install ends in, so a pass
/// that churns it would re-encode the whole library on the one launch it gets.
#[tokio::test]
async fn a_cover_already_inside_the_bounds_is_left_alone() -> TestResult {
    let library = Library::new().await?;
    let (_source_dir, source) = melodia_testkit::write_test_jpeg_sized(64, 64)?;
    // Seeded through the writer rather than copied, so the name is the content hash the store
    // actually uses. Any other name is a file no install has, and the pass re-points it for the
    // name alone — which reads as the bounds working.
    let stored = artwork::store_image(&std::fs::read(&source)?, "jpg", &library.paths.artwork_dir)
        .ok_or("store_image refused an in-bounds source")?;
    library.refer_to(Path::new(&stored)).await?;
    let before = store_contents(&library.paths.artwork_dir)?;

    renormalize(&library.db, &library.paths).await?;

    assert_eq!(
        store_contents(&library.paths.artwork_dir)?,
        before,
        "an in-bounds cover must not leave a second copy behind"
    );
    assert_eq!(library.artwork_path().await?.as_deref(), Some(stored.as_str()));
    Ok(())
}

/// A library with no artwork at all — a fresh install, or one whose covers are all embedded and
/// unscanned. The early return is what keeps the pass off the blocking pool entirely.
#[tokio::test]
async fn a_library_with_no_stored_artwork_is_a_no_op() -> TestResult {
    let library = Library::new().await?;

    renormalize(&library.db, &library.paths).await?;

    assert!(store_contents(&library.paths.artwork_dir)?.is_empty(), "nothing was there to write");
    Ok(())
}
