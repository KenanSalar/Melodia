use super::*;

// The pure-logic helpers. Everything async, and everything needing the Slint event loop, is
// covered a layer down in `services::updater`.

#[test]
fn needs_check_returns_true_when_never_checked() {
    assert!(needs_check(0));
    assert!(needs_check(-1));
}

#[test]
fn needs_check_returns_true_after_24h() {
    let now = Utc::now().timestamp();
    assert!(needs_check(now - ONE_DAY_SECS));
    assert!(needs_check(now - (ONE_DAY_SECS + 1)));
}

#[test]
fn needs_check_returns_false_inside_24h() {
    let now = Utc::now().timestamp();
    // 1 minute ago.
    assert!(!needs_check(now - 60));
    // 23h59m ago.
    assert!(!needs_check(now - (ONE_DAY_SECS - 60)));
}

#[test]
fn needs_check_returns_false_for_future_timestamp() {
    // A `last_check_unix` ahead of `now` (clock skew, NTP step)
    // produces a negative elapsed time. `needs_check` compares
    // against `ONE_DAY_SECS` and so returns false — we skip the
    // check rather than re-firing, which is the safe behavior
    // (a check from "the future" suggests an unreliable clock).
    let future = Utc::now().timestamp() + 3600;
    assert!(!needs_check(future));
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

#[test]
fn nothing_skipped_means_nothing_muted() {
    assert_eq!(
        skip_verdict("", "0.3.0", false),
        SkipVerdict {
            notify: true,
            clear_skip: false
        }
    );
}

#[test]
fn a_skip_naming_this_very_version_mutes_it() {
    assert_eq!(
        skip_verdict("0.3.0", "0.3.0", false),
        SkipVerdict {
            notify: false,
            clear_skip: false
        }
    );
}

/// The skip is a decision about one release, not a standing preference. A newer one has to get
/// through, and the stale entry goes with it so the next check doesn't re-derive the same answer.
#[test]
fn a_strictly_newer_version_clears_the_skip_and_notifies() {
    assert_eq!(
        skip_verdict("0.3.0", "0.4.0", false),
        SkipVerdict {
            notify: true,
            clear_skip: true
        }
    );
}

/// The one with a security consequence: the publisher flagged this release as not-skippable, and
/// a mute the user set weeks ago must not be what keeps it off their screen.
#[test]
fn a_critical_release_surfaces_through_a_matching_skip() {
    assert_eq!(
        skip_verdict("0.3.0", "0.3.0", true),
        SkipVerdict {
            notify: true,
            clear_skip: false
        }
    );
}

/// A stored value semver can't read would otherwise mute every notification for the life of the
/// install, since the comparison that should retire it can never succeed.
#[test]
fn a_skip_that_is_not_semver_clears_rather_than_muting_forever() {
    for stored in ["v0.3.0", "latest", "0.3"] {
        assert_eq!(
            skip_verdict(stored, "0.4.0", false),
            SkipVerdict {
                notify: true,
                clear_skip: true
            },
            "{stored:?} must not be able to mute notifications permanently"
        );
    }
}
