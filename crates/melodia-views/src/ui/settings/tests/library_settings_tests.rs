use jiff::{SignedDuration, Timestamp};

use super::format_last_scanned;
use melodia_core::utils::rfc3339;

const LIBRARY_SETTINGS: &str = include_str!("../library_settings.rs");

/// A launch scans every folder, and one that changed nothing bumps no `library_changed`, so the
/// run letting go of the bar is the only thing that moves a folder's "scanned N ago".
#[test]
fn a_finished_scan_refreshes_the_folder_list() {
    let src = melodia_testkit::strip_line_comments(LIBRARY_SETTINGS);

    let finished = melodia_testkit::block_after(&src, "if snapshot.is_none()");

    assert!(finished.contains("refresh_folders("), "a run's end refreshes nothing: {finished}");
}

#[test]
fn none_returns_empty_string() {
    assert_eq!(format_last_scanned(None), "");
}

#[test]
fn invalid_rfc3339_returns_empty_string() {
    assert_eq!(format_last_scanned(Some("not a date")), "");
}

fn stamp_ago(elapsed: SignedDuration) -> Result<String, jiff::Error> {
    Ok(rfc3339(Timestamp::now().checked_sub(elapsed)?))
}

#[test]
fn just_now_under_one_minute() -> Result<(), jiff::Error> {
    let stamp = stamp_ago(SignedDuration::from_secs(5))?;
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned just now");
    Ok(())
}

#[test]
fn minutes_ago_under_one_hour() -> Result<(), jiff::Error> {
    let stamp = stamp_ago(SignedDuration::from_mins(7))?;
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 7 minutes ago");
    Ok(())
}

#[test]
fn one_minute_singular() -> Result<(), jiff::Error> {
    let stamp = stamp_ago(SignedDuration::from_secs(75))?;
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 1 minute ago");
    Ok(())
}

#[test]
fn hours_ago_under_one_day() -> Result<(), jiff::Error> {
    let stamp = stamp_ago(SignedDuration::from_hours(3))?;
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 3 hours ago");
    Ok(())
}

#[test]
fn days_ago_under_one_week() -> Result<(), jiff::Error> {
    let stamp = stamp_ago(SignedDuration::from_hours(4 * 24))?;
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 4 days ago");
    Ok(())
}

#[test]
fn absolute_date_for_old_stamps() {
    let stamp = "2024-03-15T10:00:00Z";
    assert_eq!(format_last_scanned(Some(stamp)), "scanned 2024-03-15");
}
