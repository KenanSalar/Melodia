use super::{SkipVerdict, skip_verdict};

#[test]
fn nothing_skipped_means_nothing_muted() {
    assert_eq!(skip_verdict("", "0.3.0", false), SkipVerdict { notify: true, clear_skip: false });
}

#[test]
fn a_skip_naming_this_very_version_mutes_it() {
    assert_eq!(
        skip_verdict("0.3.0", "0.3.0", false),
        SkipVerdict { notify: false, clear_skip: false }
    );
}

/// The skip is a decision about one release, not a standing preference. A newer one has to get
/// through, and the stale entry goes with it so the next check doesn't re-derive the same answer.
#[test]
fn a_strictly_newer_version_clears_the_skip_and_notifies() {
    assert_eq!(
        skip_verdict("0.3.0", "0.4.0", false),
        SkipVerdict { notify: true, clear_skip: true }
    );
}

/// The one with a security consequence: the publisher flagged this release as not-skippable, and
/// a mute the user set weeks ago must not be what keeps it off their screen.
#[test]
fn a_critical_release_surfaces_through_a_matching_skip() {
    assert_eq!(
        skip_verdict("0.3.0", "0.3.0", true),
        SkipVerdict { notify: true, clear_skip: false }
    );
}

/// A stored value semver can't read would otherwise mute every notification for the life of the
/// install, since the comparison that should retire it can never succeed.
#[test]
fn a_skip_that_is_not_semver_clears_rather_than_muting_forever() {
    for stored in ["v0.3.0", "latest", "0.3"] {
        assert_eq!(
            skip_verdict(stored, "0.4.0", false),
            SkipVerdict { notify: true, clear_skip: true },
            "{stored:?} must not be able to mute notifications permanently"
        );
    }
}
