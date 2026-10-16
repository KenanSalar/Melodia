use super::*;

// The pure-logic helpers. Everything async, and everything needing the Slint event loop, is
// covered a layer down in `services::updater`.

/// The instant the table below is read at.
const NOW: i64 = 1_760_000_000;

/// A day to the second, and a second either side of it. A stamp ahead of now is a clock stepped
/// back, which waits rather than checking again.
#[test]
fn a_check_is_due_from_exactly_one_day_after_the_last() {
    let rows = [
        ("never checked", 0, true),
        ("a stamp before the epoch", -1, true),
        ("exactly a day ago", NOW - ONE_DAY_SECS, true),
        ("a second short of a day", NOW - ONE_DAY_SECS + 1, false),
        ("a second past a day", NOW - ONE_DAY_SECS - 1, true),
        ("a minute ago", NOW - 60, false),
        ("an hour ahead", NOW + 3_600, false),
    ];
    for (what, last_check, due) in rows {
        assert_eq!(needs_check(last_check, NOW), due, "{what}");
    }
}

#[test]
fn backoff_delay_for_zero_failures_is_normal_cadence() {
    assert_eq!(backoff_delay_for(0), NORMAL_CADENCE);
}

#[test]
fn backoff_delay_for_one_failure_is_normal_cadence() {
    // Single hiccup shouldn't extend the cadence — only repeated
    // failures back off.
    assert_eq!(backoff_delay_for(1), NORMAL_CADENCE);
}

#[test]
fn backoff_delay_for_climbs_the_ladder() {
    assert_eq!(backoff_delay_for(2), Duration::from_hours(12));
    assert_eq!(backoff_delay_for(3), Duration::from_hours(24));
    assert_eq!(backoff_delay_for(4), Duration::from_hours(7 * 24));
}

#[test]
fn backoff_delay_for_caps_at_seven_days() {
    // Years of consecutive failures shouldn't blow past the
    // ladder; the saturating add on the counter + ladder index
    // clamp keep us at the 7d ceiling forever.
    assert_eq!(backoff_delay_for(u8::MAX), Duration::from_hours(7 * 24));
    assert_eq!(backoff_delay_for(100), Duration::from_hours(7 * 24));
}
