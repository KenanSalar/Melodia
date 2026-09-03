//! The two gates that keep the sweep off files it shouldn't touch.
//!
//! Both failure modes are silent and permanent: a name the predicate wrongly accepts is a user's
//! own file deleted, and a referenced file wrongly swept is a cover gone until a re-scan.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use super::super::is_stored_name;
use super::super::sweep::{SweepReport, collect_candidates, retire};
use melodia_core::error::AppError;

/// Long enough that nothing a test writes is ever old enough to sweep on its own.
const HOUR: Duration = Duration::from_hours(1);

/// A clock well past anything a test writes, so `now - mtime` clears the grace window.
fn well_past_grace() -> SystemTime {
    SystemTime::now() + Duration::from_hours(48)
}

/// Both halves in the order the caller runs them. Nothing is committing rows under a test, so
/// having the reference set in hand first costs the ordering nothing here.
fn sweep<S: std::hash::BuildHasher>(
    dir: &std::path::Path,
    referenced: &HashSet<String, S>,
    grace: Duration,
    now: SystemTime,
) -> SweepReport {
    let (candidates, report) = collect_candidates(dir, grace, now);
    retire(candidates, referenced, report)
}

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<(), AppError> {
    std::fs::write(dir.join(name), bytes)?;
    Ok(())
}

/// The store is a folder in the user's data directory, so the predicate is the whole of what
/// keeps the sweep off a file this module did not write.
#[test]
fn the_predicate_accepts_only_names_the_writers_produce() {
    for name in [
        "33fb807d1f1b7cbb.jpg",
        "abcdef0123456789.png",
        "0000000000000000.webp",
        "ffffffffffffffff.tiff",
        // Builds before `stored_extension` folded the case took the picked file's own, so a
        // store shipped by one of them holds these and they are as much ours to retire.
        "abcdef0123456789.JPG",
        "abcdef0123456789.PNG",
    ] {
        assert!(is_stored_name(name), "{name} is exactly what the writers emit");
    }

    for name in [
        "README",                   // no extension at all
        "mycover.jpg",              // a name a user chose
        "33fb807d1f1b7cbb",         // the hash without one
        "33fb807d1f1b7cbb.jpg.tmp", // a staged write that never landed
        "33fb807d1f1b7cb.jpg",      // fifteen hex chars
        "33fb807d1f1b7cbbb.jpg",    // seventeen
        "33FB807D1F1B7CBB.jpg",     // an upper-case stem, which `to_hex` never produces
        "33fb807d1f1b7cbg.jpg",     // not hex
        "33fb807d1f1b7cbb.txt",     // not an image extension we write
    ] {
        assert!(!is_stored_name(name), "{name} is not ours to delete");
    }
}

/// The case the four-column reference query exists for: a composite pointed at by a playlist and
/// nothing else. If this regresses, every custom playlist mosaic in the app goes blank.
#[test]
fn a_referenced_file_survives_and_an_orphan_does_not() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();

    write(dir, "33fb807d1f1b7cbb.jpg", b"a playlist composite")?;
    write(dir, "4cccaf4d4b4cea11.jpg", b"an orphan")?;
    write(dir, "notes.txt", b"a file the user put here")?;

    let referenced: HashSet<String> = ["33fb807d1f1b7cbb.jpg".to_owned()].into_iter().collect();
    let report = sweep(dir, &referenced, HOUR, well_past_grace());

    assert!(dir.join("33fb807d1f1b7cbb.jpg").exists(), "a referenced composite must survive");
    assert!(!dir.join("4cccaf4d4b4cea11.jpg").exists(), "an unreferenced file must go");
    assert!(dir.join("notes.txt").exists(), "a name we never wrote is not ours to delete");
    assert_eq!(
        report,
        SweepReport {
            deleted: 1,
            bytes: 9,
            kept: 1,
            failed: 0,
        }
    );
    Ok(())
}

/// The reference set is read *between* the two halves in production, so a file that becomes
/// referenced while the directory is being listed still has to survive — which is the whole reason
/// the listing goes first.
#[test]
fn a_candidate_referenced_after_the_listing_is_still_spared() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    write(dir, "abcdef0123456789.png", b"carried over from an earlier session")?;

    // Past the grace window, so nothing but the reference set can save it — the shape of a cover
    // `store_image` found already on disk and so never re-stamped.
    let (candidates, report) = collect_candidates(dir, HOUR, well_past_grace());
    assert_eq!(candidates.len(), 1);

    // The row a concurrent scan committed while the listing ran.
    let referenced: HashSet<String> = ["abcdef0123456789.png".to_owned()].into_iter().collect();
    let report = retire(candidates, &referenced, report);

    assert!(dir.join("abcdef0123456789.png").exists(), "the later commit has to win");
    assert_eq!(report.deleted, 0);
    Ok(())
}

/// A tag edit or scan worker can have written a cover whose row hasn't committed. Without the
/// window the sweep deletes it and leaves the committing transaction pointing at nothing.
#[test]
fn the_grace_window_keeps_a_just_written_file() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    write(dir, "abcdef0123456789.png", b"written a moment ago")?;

    // The clock is injected rather than slept on, so this is the real elapsed-time comparison
    // with no wall-clock cost.
    let report = sweep(dir, &HashSet::new(), HOUR, SystemTime::now());

    assert!(dir.join("abcdef0123456789.png").exists(), "inside the window, nothing goes");
    assert_eq!(report.deleted, 0);
    assert_eq!(report.kept, 1);
    Ok(())
}

/// The same file, once it is old enough — so the test above is pinning the window rather than a
/// sweep that never deletes anything.
#[test]
fn the_same_orphan_goes_once_it_is_past_the_window() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    write(dir, "abcdef0123456789.png", b"written a moment ago")?;

    let report = sweep(dir, &HashSet::new(), HOUR, well_past_grace());

    assert!(!dir.join("abcdef0123456789.png").exists());
    assert_eq!(report.deleted, 1);
    Ok(())
}

/// A missing store is a fresh install or a directory the user moved; neither is an error worth
/// failing a scan over.
#[test]
fn a_missing_directory_sweeps_nothing_and_does_not_panic() {
    let report = sweep(
        std::path::Path::new("/nonexistent/melodia/artwork"),
        &HashSet::new(),
        HOUR,
        SystemTime::now(),
    );
    assert_eq!(report, SweepReport::default());
}
