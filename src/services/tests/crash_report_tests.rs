use std::path::Path;

use chrono::{Local, TimeZone};

use super::{MAX_CRASH_REPORTS, file_name, format_report, prune, recent, timestamp_of};
use crate::error::AppError;
use crate::test_support::{reading_env, with_env_var};

/// Everything in `dir`, sorted. Report names are fixed-width, so lexicographic
/// order is chronological order — the same property retention leans on.
fn entry_names(dir: &Path) -> Result<Vec<String>, AppError> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

/// A local timestamp `n` seconds past an arbitrary fixed instant, so the tests
/// order reports without depending on the wall clock.
fn stamp(offset_secs: i64) -> chrono::DateTime<Local> {
    let base = Local
        .with_ymd_and_hms(2026, 8, 6, 14, 30, 0)
        .single()
        .unwrap_or_else(Local::now);
    base + chrono::TimeDelta::seconds(offset_secs)
}

/// The report is what a reporter attaches, so every field earning a line in the
/// format has to actually appear in the output.
///
/// Under `reading_env` because the header reaches `$HOME`, the two XDG session
/// variables and — through `current_target_key()` — `$APPIMAGE`, and a reader
/// races a sibling's mutation exactly as a second mutator would.
#[test]
fn a_report_carries_every_field() {
    let report = reading_env(|| {
        format_report(
            stamp(0),
            Some("melodia-bg"),
            Some("src/ui/foo.rs:123"),
            "assertion failed",
            "  0: melodia::ui::foo::bar",
        )
    });

    for expected in [
        "Melodia crash report",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        "melodia-bg",
        "src/ui/foo.rs:123",
        "assertion failed",
        "backtrace",
        "melodia::ui::foo::bar",
    ] {
        assert!(report.contains(expected), "report is missing {expected:?}");
    }
}

/// A panic can happen on an unnamed thread, without a location, or with a
/// payload that isn't a string. None of those may cost the report its shape.
#[test]
fn a_report_survives_missing_fields() {
    let report = reading_env(|| format_report(stamp(0), None, None, "boom", ""));
    assert!(report.contains("<unnamed>"));
    assert!(report.contains("<unknown>"));
    assert!(report.contains("boom"));
}

/// The report goes into a public issue, so the home directory — which usually
/// holds a real name — must not ride along in the payload or the backtrace.
#[test]
fn a_report_redacts_the_home_directory() {
    with_env_var("HOME", Some("/home/testuser"), || {
        let report = format_report(
            stamp(0),
            Some("main"),
            Some("src/lib.rs:1"),
            "failed to open /home/testuser/Music/x.flac",
            "  0: at /home/testuser/Development/Melodia/src/lib.rs",
        );

        assert!(!report.contains("/home/testuser"), "home leaked: {report}");
        assert!(report.contains("~/Music/x.flac"));
        assert!(report.contains("~/Development/Melodia"));
    });
}

/// `file_name` and `timestamp_of` are one scheme and its inverse; pruning is
/// gated on the latter, so a drift between them silently disarms the sweep.
#[test]
fn a_name_round_trips_through_its_inverse() {
    let now = stamp(0);
    let parsed = timestamp_of(&file_name(now));
    assert_eq!(parsed, Some(now.naive_local()));
}

/// Written out of order on purpose: a prune that trusted `read_dir` order rather
/// than the timestamp in the name would pass on a sorted directory.
#[test]
fn prune_keeps_the_newest_and_deletes_the_rest() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let offsets: Vec<i64> = (0..i64::try_from(MAX_CRASH_REPORTS).unwrap_or(10) + 3).collect();
    for offset in offsets.iter().rev() {
        std::fs::write(tmp.path().join(file_name(stamp(*offset))), b"x")?;
    }

    prune(tmp.path());

    let kept = entry_names(tmp.path())?;
    assert_eq!(kept.len(), MAX_CRASH_REPORTS);
    // The three oldest are the ones that go.
    for offset in &offsets[..3] {
        assert!(
            !kept.contains(&file_name(stamp(*offset))),
            "prune kept an old report"
        );
    }
    Ok(())
}

/// The safety property: pruning is gated on parsing a name back into the scheme
/// this module writes, not on the file merely sitting in the directory. The log
/// files share that directory, so the mutation to catch is a sweep that deletes
/// by age or by count alone.
#[test]
fn prune_never_touches_a_name_it_did_not_write() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let decoys = [
        "melodia_rCURRENT.log",
        "melodia_r00000.log",
        "crash-NOPE.txt",
        "crash-20260806-143000.log",
        "notes.txt",
    ];
    for name in decoys {
        std::fs::write(tmp.path().join(name), b"x")?;
    }
    for offset in 0..i64::try_from(MAX_CRASH_REPORTS).unwrap_or(10) + 5 {
        std::fs::write(tmp.path().join(file_name(stamp(offset))), b"x")?;
    }

    prune(tmp.path());

    for name in decoys {
        assert!(tmp.path().join(name).exists(), "prune deleted {name}");
    }
    Ok(())
}

/// The diagnostics bundle wants the crash the user just had, not the first one
/// they ever had.
#[test]
fn recent_returns_the_newest_first() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    for offset in 0..5 {
        std::fs::write(tmp.path().join(file_name(stamp(offset))), b"x")?;
    }

    let newest = recent(tmp.path(), 2);

    let names: Vec<String> = newest
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
        .collect();
    assert_eq!(names, [file_name(stamp(4)), file_name(stamp(3))]);
    Ok(())
}

/// A directory with no reports in it — or none at all — is the normal case, not
/// an error: most runs never panic.
#[test]
fn an_empty_folder_yields_nothing() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    assert!(recent(tmp.path(), 5).is_empty());
    assert!(recent(&tmp.path().join("missing"), 5).is_empty());
    prune(&tmp.path().join("missing"));
    Ok(())
}
