//! Which directories a pass walks, and which window each entry point spends.
//!
//! The gates themselves are pinned a layer down in `melodia-artwork`, and the reference set a
//! layer sideways in `queries::artwork`. What only this layer can answer is what the two entry
//! points *spend* them on, and both failures are silent: a store dropped from `run`'s vec leaks
//! forever, and the two grace windows swapped either retire a logo whose cache row has not
//! committed or leave `radio-logos/` growing for the rest of the session.
//!
//! Every case turns on a station logo, that being the one thing in the three stores a re-scan
//! cannot put back.

use std::path::Path;
use std::time::SystemTime;

use tempfile::TempDir;

use super::*;
use melodia_core::entities::radio;

/// Older than either window, so nothing but the reference set can save a file aged with it.
const PAST_EVERY_WINDOW: Duration = Duration::from_hours(2);

/// A library rooted in a temp dir, with all three stores created — `collect_candidates` sweeps
/// nothing on a missing directory, so an assertion against one would pass without this.
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
        Ok(Self {
            db: DbPool::test_pool().await?,
            paths,
            _tmp: tmp,
        })
    }
}

/// A stored file in `dir`, backdated by `age`. `sweep_stores` takes its own `SystemTime::now()`,
/// so the file's mtime is the only handle this layer has on either window.
///
/// The name is spelled to satisfy `is_stored_name` rather than asked: that predicate is
/// `pub(super)` in `melodia-artwork` and has its own suite there.
fn aged(dir: &Path, name: &str, age: Duration) -> Result<PathBuf, AppError> {
    let path = dir.join(name);
    std::fs::write(&path, b"stored artwork")?;
    std::fs::File::options().write(true).open(&path)?.set_modified(SystemTime::now() - age)?;
    Ok(path)
}

/// A station row naming the logo — one of the two columns that can hold one alive, the other
/// being its browse cache row.
async fn station_with_logo(db: &DbPool, artwork_path: &Path) -> Result<(), AppError> {
    let station = radio::NewRadioStation {
        name: "Test Station".to_owned(),
        stream_url: "http://example.invalid/stream".to_owned(),
        ..Default::default()
    };
    let id = queries::radio::save_station(db, &station).await?;
    queries::radio::set_artwork(db, id, Some(&artwork_path.to_string_lossy())).await
}

/// One store dropped from `run`'s vec leaks every orphan in it forever, and shows up nowhere:
/// the pass still returns `Ok` and still logs the other two.
#[tokio::test]
async fn a_pass_reaches_every_store_the_paths_name() -> Result<(), AppError> {
    let library = Library::new().await?;
    let orphans = [
        aged(&library.paths.artwork_dir, "33fb807d1f1b7cbb.jpg", PAST_EVERY_WINDOW)?,
        aged(&library.paths.artists_dir, "4cccaf4d4b4cea11.jpg", PAST_EVERY_WINDOW)?,
        aged(&library.paths.radio_logos_dir, "5dd0bf5e5c5dfb22.png", PAST_EVERY_WINDOW)?,
    ];

    run(&library.db, &library.paths).await?;

    for orphan in &orphans {
        assert!(!orphan.exists(), "{} survived a pass nothing referenced it in", orphan.display());
    }
    Ok(())
}

/// Logos lived in `artwork/` before they had a directory, and the ones already on disk stayed
/// there. One reference set over however many directories is the whole of what keeps them: the
/// row names the file wherever it sits, so introducing `radio-logos/` needed no pass to move
/// them and none to protect them.
#[tokio::test]
async fn a_logo_that_predates_its_own_directory_is_still_held_by_its_row() -> Result<(), AppError> {
    let library = Library::new().await?;
    let logo = aged(&library.paths.artwork_dir, "6ee0cf6f6d6e0c33.png", PAST_EVERY_WINDOW)?;
    station_with_logo(&library.db, &logo).await?;

    run(&library.db, &library.paths).await?;

    assert!(logo.exists(), "a station's own logo is not an orphan for sitting in the old store");
    Ok(())
}

/// The reason there is a second entry point at all: it runs whenever Radio is done with, on a
/// schedule that has nothing to do with a scan, and one directory rather than three is what
/// keeps that cheap.
#[tokio::test]
async fn the_logo_entry_point_leaves_the_other_two_stores_alone() -> Result<(), AppError> {
    let library = Library::new().await?;
    let cover = aged(&library.paths.artwork_dir, "33fb807d1f1b7cbb.jpg", PAST_EVERY_WINDOW)?;
    let artist = aged(&library.paths.artists_dir, "4cccaf4d4b4cea11.jpg", PAST_EVERY_WINDOW)?;
    let logo = aged(&library.paths.radio_logos_dir, "5dd0bf5e5c5dfb22.png", PAST_EVERY_WINDOW)?;

    run_radio_logos(&library.db, &library.paths).await?;

    assert!(cover.exists(), "a cover is retired after a scan, which this is not");
    assert!(artist.exists(), "and so is an artist image");
    assert!(!logo.exists(), "the one store this entry point is for");
    Ok(())
}

/// Both sides of the real constant. Under it a logo may be one whose cache row is still queued
/// behind a scan chunk on the single write connection; over it, one the retention pass has
/// already dropped and nothing will ever name again.
#[tokio::test]
async fn the_short_window_decides_a_logo_by_the_second() -> Result<(), AppError> {
    for (age, survives) in [
        (RADIO_GRACE.saturating_sub(Duration::from_secs(1)), true),
        (RADIO_GRACE.saturating_add(Duration::from_secs(1)), false),
    ] {
        let library = Library::new().await?;
        let logo = aged(&library.paths.radio_logos_dir, "7ff0df707e7f1d44.png", age)?;

        run_radio_logos(&library.db, &library.paths).await?;

        assert_eq!(logo.exists(), survives, "a logo aged {age:?} against a {RADIO_GRACE:?} window");
    }
    Ok(())
}

/// The asymmetry, over the one directory both entry points reach. Half an hour is past
/// `RADIO_GRACE` and well inside `GRACE`, so the same file has two answers depending on which
/// pass ran — and the scan pass is the one that must not take it, its window covering a
/// transaction rather than a write-pool hop.
#[tokio::test]
async fn the_scan_pass_keeps_a_logo_the_logo_pass_would_have_retired() -> Result<(), AppError> {
    let library = Library::new().await?;
    let logo =
        aged(&library.paths.radio_logos_dir, "8001ef818f801e55.png", Duration::from_mins(30))?;

    run(&library.db, &library.paths).await?;

    assert!(logo.exists(), "the scan pass spends {GRACE:?}, not {RADIO_GRACE:?}");
    Ok(())
}
