//! The order the two halves run in, which is the whole of what this module decides.
//!
//! Both halves are pinned already: `queries::radio::prune_logo_answers` has its own suite for the
//! bounds, and `artwork_sweep` has one for the windows and the stores. What neither can see is
//! the sequence, and the module doc argues it in prose: prune first, so the rows stop referencing
//! their files, then sweep. Reversed, every dropped row's file still counts as referenced when
//! the sweep looks, and `radio-logos/` grows for the rest of the install.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tempfile::TempDir;

use super::*;
use melodia_store::database::queries;

/// Older than `RADIO_GRACE`, so nothing but the reference set can keep a file aged with it.
const PAST_THE_GRACE: std::time::Duration = std::time::Duration::from_hours(2);

/// Long enough ago that `LOGO_CACHE_MAX_AGE_DAYS` has passed however this run is dated.
const LONG_SPENT: &str = "2020-01-01T00:00:00.000+00:00";

/// Answered just now, so the same pass keeps it.
fn answered_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

struct Store {
    db: DbPool,
    paths: Paths,
    _tmp: TempDir,
}

impl Store {
    async fn new() -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let paths = Paths::rooted_at(tmp.path().to_path_buf());
        paths.create_dirs()?;
        Ok(Self {
            db: DbPool::test_pool().await?,
            paths,
            _tmp: tmp,
        })
    }

    /// A cached logo on disk plus the answer row naming it, `answered_at` deciding whether the
    /// prune is going to drop that row.
    async fn cached_logo(&self, name: &str, answered_at: &str) -> Result<PathBuf, AppError> {
        let path = self.paths.radio_logos_dir.join(name);
        std::fs::write(&path, b"stored logo")?;
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(SystemTime::now() - PAST_THE_GRACE)?;

        queries::radio::record_logo_hit(
            &self.db,
            &format!("http://example.invalid/{name}"),
            &path.to_string_lossy(),
            i64::try_from(b"stored logo".len()).unwrap_or(i64::MAX),
            answered_at,
        )
        .await?;
        Ok(path)
    }
}

fn exists(path: &Path) -> bool {
    path.try_exists().unwrap_or(false)
}

/// Sweeping before pruning leaves the file behind, because the row that is about to be dropped
/// still names it. Nothing else in the tree notices: the pass returns `Ok` either way and the
/// store just never shrinks.
#[tokio::test]
async fn a_logo_whose_answer_expired_is_gone_by_the_end_of_one_pass() -> Result<(), AppError> {
    let store = Store::new().await?;
    let logo = store.cached_logo("aa11bb22cc33dd44.png", LONG_SPENT).await?;

    run(&store.db, &store.paths).await?;

    assert!(!exists(&logo), "the answer aged out, so nothing references the file any more");
    Ok(())
}

/// The other side of the same pass, and what stops the first case passing for a sweep that
/// deletes whatever it finds: a live answer holds its file through both halves.
#[tokio::test]
async fn a_logo_whose_answer_is_still_current_survives_the_pass() -> Result<(), AppError> {
    let store = Store::new().await?;
    let logo = store.cached_logo("ee55ff66aa77bb88.png", &answered_now()).await?;

    run(&store.db, &store.paths).await?;

    assert!(exists(&logo), "the cache row still names this file, so the sweep may not retire it");
    Ok(())
}
