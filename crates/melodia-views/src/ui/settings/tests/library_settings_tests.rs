use super::format_last_scanned;

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

#[test]
fn just_now_under_one_minute() {
    let stamp = (chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned just now");
}

#[test]
fn minutes_ago_under_one_hour() {
    let stamp = (chrono::Utc::now() - chrono::Duration::minutes(7)).to_rfc3339();
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 7 minutes ago");
}

#[test]
fn one_minute_singular() {
    let stamp = (chrono::Utc::now() - chrono::Duration::seconds(75)).to_rfc3339();
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 1 minute ago");
}

#[test]
fn hours_ago_under_one_day() {
    let stamp = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 3 hours ago");
}

#[test]
fn days_ago_under_one_week() {
    let stamp = (chrono::Utc::now() - chrono::Duration::days(4)).to_rfc3339();
    assert_eq!(format_last_scanned(Some(&stamp)), "scanned 4 days ago");
}

#[test]
fn absolute_date_for_old_stamps() {
    let stamp = "2024-03-15T10:00:00Z";
    assert_eq!(format_last_scanned(Some(stamp)), "scanned 2024-03-15");
}
