//! When the picker asks `AutoEq` whether its list moved: once a day behind a list it can show, and
//! at once behind one it can't.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::cached_list;
use melodia_core::config::Paths;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ENTRY_LINE: &str = "- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650)\n";

const CHECKED_AT_SECS: u64 = 1_800_000_000;

const DAY: Duration = Duration::from_hours(24);

fn checked_at() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(CHECKED_AT_SECS)
}

/// A data root holding the cached list's `text`, if any, last checked at [`CHECKED_AT_SECS`].
fn cache_root(text: Option<&str>) -> std::io::Result<(tempfile::TempDir, Paths)> {
    let tmp = tempfile::tempdir()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    let meta = format!(r#"{{"etag": "\"list-tag\"", "checked_at_secs": {CHECKED_AT_SECS}}}"#);
    std::fs::write(&paths.autoeq_list_meta_path, meta)?;
    if let Some(text) = text {
        std::fs::write(&paths.autoeq_list_path, text)?;
    }
    Ok((tmp, paths))
}

#[test]
fn a_list_the_picker_can_show_is_rechecked_a_day_after_its_last_check() -> TestResult {
    let (_tmp, paths) = cache_root(Some(ENTRY_LINE))?;
    let check = cached_list(&paths).check;

    let cases = [
        ("just checked", checked_at(), false),
        ("a second short of a day", checked_at() + DAY - Duration::from_secs(1), false),
        ("a day on", checked_at() + DAY, true),
    ];
    for (when, now, due) in cases {
        assert_eq!(check.is_due(now), due, "{when}");
    }
    Ok(())
}

/// The picker otherwise sits on "couldn't be read" until tomorrow, behind a cache a recheck could
/// have replaced at once.
#[test]
fn a_cache_with_no_list_to_show_is_due_at_once() -> TestResult {
    let cases = [("no list on disk", None), ("a list with no entry", Some("# AutoEq\n"))];
    for (what, text) in cases {
        let (_tmp, paths) = cache_root(text)?;

        let check = cached_list(&paths).check;

        assert!(check.is_due(checked_at()), "{what}");
    }
    Ok(())
}
