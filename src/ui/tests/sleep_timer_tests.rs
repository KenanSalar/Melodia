use super::{clamp_sleep_seconds, format_remaining, MAX_SLEEP_SECONDS, MIN_SLEEP_SECONDS};

#[test]
fn format_remaining_pads_seconds() {
    assert_eq!(format_remaining(0), "0:00");
    assert_eq!(format_remaining(5), "0:05");
    assert_eq!(format_remaining(59), "0:59");
}

#[test]
fn format_remaining_splits_minutes() {
    assert_eq!(format_remaining(60), "1:00");
    assert_eq!(format_remaining(90), "1:30");
    assert_eq!(format_remaining(15 * 60), "15:00");
    assert_eq!(format_remaining(59 * 60 + 59), "59:59");
}

#[test]
fn format_remaining_uses_hours_past_an_hour() {
    assert_eq!(format_remaining(3600), "1:00:00");
    assert_eq!(format_remaining(90 * 60), "1:30:00");
    assert_eq!(format_remaining(2 * 3600), "2:00:00");
    assert_eq!(format_remaining(3600 + 5 * 60 + 7), "1:05:07");
}

#[test]
fn clamp_sleep_seconds_enforces_bounds() {
    assert_eq!(clamp_sleep_seconds(MIN_SLEEP_SECONDS), MIN_SLEEP_SECONDS);
    assert_eq!(clamp_sleep_seconds(MAX_SLEEP_SECONDS), MAX_SLEEP_SECONDS);
    assert_eq!(clamp_sleep_seconds(600), 600);
    // Below the floor / above the ceiling / negative all clamp.
    assert_eq!(clamp_sleep_seconds(0), MIN_SLEEP_SECONDS);
    assert_eq!(clamp_sleep_seconds(5), MIN_SLEEP_SECONDS);
    assert_eq!(clamp_sleep_seconds(-100), MIN_SLEEP_SECONDS);
    assert_eq!(clamp_sleep_seconds(999_999), MAX_SLEEP_SECONDS);
}
