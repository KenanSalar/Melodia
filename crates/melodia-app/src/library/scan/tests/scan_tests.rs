//! Which files under a folder its own scan reads, and which of its rows the scan purges.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::{Discovery, ScanRun, ScanScope, discover};
use crate::library::settings::folders::NestedFolder;
use crate::state::ScanControl;
use melodia_core::error::AppError;
use melodia_store::database::queries::fixtures::{insert_tagged_track, make_test_metadata};
use melodia_store::database::{DbPool, queries};
use melodia_store::media::ingest::metadata::date_modified_from_metadata;

/// A library folder nested inside this one is scanned by its own scan until a completed one here
/// absorbs it. The folder an import filed loose tracks under is scanned by nothing, so its files
/// are read here. Compared by component: a sibling sharing a nested folder's name as a prefix is
/// this scan's, or nothing would read it.
#[test]
fn a_scan_reads_everything_under_it_but_a_nested_library_folder() {
    let root = PathBuf::from("music");
    let scope = ScanScope {
        root: root.clone(),
        nested: vec![
            NestedFolder { id: 2, path: root.join("rock"), is_enabled: true },
            NestedFolder { id: 3, path: root.join("imported"), is_enabled: false },
        ],
    };
    let cases = [
        (root.join("a.mp3"), true),
        (root.join("rock").join("b.mp3"), false),
        (root.join("imported").join("c.mp3"), true),
        (root.join("rockabilly").join("d.mp3"), true),
    ];

    for (path, owned) in cases {
        assert_eq!(scope.owns(&path), owned, "{}", path.display());
    }
}

/// A library holding one folder, `root`, which every row below is filed under.
async fn library_at(root: &Path) -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, &root.to_string_lossy(), true).await?;
    Ok(db)
}

/// An audio file at `path`, and the row a scan would have stored for it.
async fn scanned_file(db: &DbPool, path: &Path) -> Result<(), AppError> {
    std::fs::write(path, b"fake audio")?;
    let on_disk = std::fs::metadata(path)?;
    let mut meta = make_test_metadata(&path.to_string_lossy());
    meta.file_size = i64::try_from(on_disk.len()).unwrap_or(i64::MAX);
    meta.date_modified = date_modified_from_metadata(&on_disk);
    insert_tagged_track(db, &path.to_string_lossy(), &meta).await?;
    Ok(())
}

/// A row for `path`, which holds no file.
async fn row_without_a_file(db: &DbPool, path: &Path) -> Result<(), AppError> {
    let meta = make_test_metadata(&path.to_string_lossy());
    insert_tagged_track(db, &path.to_string_lossy(), &meta).await?;
    Ok(())
}

fn run() -> (Arc<ScanControl>, Arc<ScanRun>) {
    let control = Arc::new(ScanControl::new(&CancellationToken::new()));
    let run = ScanRun::start(&control, control.token());
    (control, run)
}

/// What a scan of `root`, holding no library folder of its own, finds there.
async fn discovered(db: &DbPool, root: &Path) -> Result<Discovery, AppError> {
    discovered_in(db, ScanScope { root: root.to_path_buf(), nested: Vec::new() }).await
}

async fn discovered_in(db: &DbPool, scope: ScanScope) -> Result<Discovery, AppError> {
    let (_control, run) = run();
    discover(db, 1, Arc::new(scope), run)
        .await?
        .ok_or_else(|| AppError::validation_msg("a scan nothing cancelled came back stopped"))
}

/// A launch scans every folder, so this is the common case, and it reads nothing only while the
/// path a row was stored under is the one the walk spells.
#[tokio::test]
async fn a_rescan_of_an_unchanged_file_reads_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = library_at(tmp.path()).await?;
    scanned_file(&db, &tmp.path().join("a.mp3")).await?;

    let found = discovered(&db, tmp.path()).await?;

    assert_eq!((found.walked, found.to_scan), (1, Vec::<PathBuf>::new()), "(walked, to read)");
    Ok(())
}

/// The other half of the common case: an unchanged file whose row the walk didn't pair would be
/// purged, rating and play count with it.
#[tokio::test]
async fn a_rescan_of_an_unchanged_file_purges_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = library_at(tmp.path()).await?;
    scanned_file(&db, &tmp.path().join("a.mp3")).await?;

    let found = discovered(&db, tmp.path()).await?;

    assert_eq!(found.orphans, Vec::<String>::new());
    Ok(())
}

/// A nested library folder's files are left to its own scan, but rows filed under this folder can
/// still sit in its directory, and the purge checks them against the whole walk. A file of this
/// folder's own keeps the walk from coming back empty, which would purge nothing for another
/// reason.
#[tokio::test]
async fn a_row_inside_a_nested_folders_directory_is_not_an_orphan() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = library_at(tmp.path()).await?;
    let nested = tmp.path().join("sub");
    std::fs::create_dir(&nested)?;
    scanned_file(&db, &tmp.path().join("a.mp3")).await?;
    scanned_file(&db, &nested.join("x.mp3")).await?;
    let scope = ScanScope {
        root: tmp.path().to_path_buf(),
        nested: vec![NestedFolder { id: 2, path: nested, is_enabled: true }],
    };

    let found = discovered_in(&db, scope).await?;

    assert_eq!(found.orphans, Vec::<String>::new());
    Ok(())
}

#[tokio::test]
async fn a_row_whose_file_left_the_folder_is_an_orphan() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = library_at(tmp.path()).await?;
    scanned_file(&db, &tmp.path().join("a.mp3")).await?;
    let gone = tmp.path().join("gone.mp3");
    row_without_a_file(&db, &gone).await?;

    let found = discovered(&db, tmp.path()).await?;

    assert_eq!(found.orphans, [gone.to_string_lossy()]);
    Ok(())
}

/// A walk that found nothing is as likely an unmounted drive as an emptied folder, and the purge
/// it would hand back is the whole folder.
#[tokio::test]
async fn an_empty_walk_names_no_orphan() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = library_at(tmp.path()).await?;
    row_without_a_file(&db, &tmp.path().join("gone.mp3")).await?;

    let found = discovered(&db, tmp.path()).await?;

    assert_eq!(found.orphans, Vec::<String>::new());
    Ok(())
}

/// A partial walk would read as a folder missing whatever the stop left unwalked.
#[tokio::test]
async fn a_cancelled_discovery_hands_nothing_back() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = library_at(tmp.path()).await?;
    scanned_file(&db, &tmp.path().join("a.mp3")).await?;
    let (control, run) = run();
    control.cancel();
    let scope = ScanScope { root: tmp.path().to_path_buf(), nested: Vec::new() };

    let found = discover(&db, 1, Arc::new(scope), run).await?;

    assert!(found.is_none(), "a cancelled scan handed back a discovery");
    Ok(())
}
