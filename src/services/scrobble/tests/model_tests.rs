use super::scrobble_threshold_ms;

#[test]
fn unknown_duration_falls_back_to_four_minutes() {
    assert_eq!(scrobble_threshold_ms(0), Some(240_000));
}

#[test]
fn tracks_at_or_under_thirty_seconds_never_scrobble() {
    assert_eq!(scrobble_threshold_ms(30_000), None);
    assert_eq!(scrobble_threshold_ms(15_000), None);
    assert_eq!(scrobble_threshold_ms(1), None);
}

#[test]
fn just_over_thirty_seconds_uses_half() {
    assert_eq!(scrobble_threshold_ms(30_001), Some(15_000));
}

#[test]
fn medium_track_uses_half_its_duration() {
    assert_eq!(scrobble_threshold_ms(100_000), Some(50_000));
}

#[test]
fn long_track_caps_at_four_minutes() {
    // Half of 8 minutes is exactly the 4-minute cap.
    assert_eq!(scrobble_threshold_ms(480_000), Some(240_000));
    assert_eq!(scrobble_threshold_ms(600_000), Some(240_000));
}
