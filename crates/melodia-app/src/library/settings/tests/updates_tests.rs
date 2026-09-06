//! The auto-updater flags. Each setter in `super::updates::*` is a `mutate_settings` closure
//! over `UpdateFlags`, and what the two substantive ones do to the struct is the struct's own
//! method — so that is what these drive, rather than a temp `Paths` and a settings file per
//! assertion.

use crate::services::settings::UpdateFlags;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn defaults_are_safe_for_new_install() {
    // First-launch users land with auto-check on (consumer-music-player
    // convention) and every counter zeroed. The plan calls these the
    // "fail-safe defaults" — no skip pinned to a stale version, no
    // etag pre-seeded so the very first check always fetches a body.
    let flags = UpdateFlags::default();
    assert!(flags.auto_check_enabled, "auto_check_enabled should default true");
    assert_eq!(flags.last_check_unix, 0);
    assert_eq!(flags.last_known_release, "");
    assert_eq!(flags.skipped_release, "");
    assert_eq!(flags.last_manifest_etag, "");
    assert_eq!(flags.consecutive_failures, 0);
}

/// At the cap the counter must stay put rather than wrapping to 0 and re-arming the 6h cadence
/// after years of failed checks.
#[test]
fn failure_counter_saturates_at_u8_max() {
    let mut flags = UpdateFlags {
        consecutive_failures: u8::MAX,
        ..UpdateFlags::default()
    };
    flags.record_failure(1_717_243_200);
    assert_eq!(flags.consecutive_failures, u8::MAX);
}

/// Without the timestamp moving, the daily loop's 24h gate never opens again and every iteration
/// re-fires the failing check instead of backing off.
#[test]
fn a_failure_advances_the_clock_as_well_as_the_counter() {
    let mut flags = UpdateFlags::default();
    flags.record_failure(1_717_243_200);
    assert_eq!(flags.consecutive_failures, 1);
    assert_eq!(flags.last_check_unix, 1_717_243_200);
}

/// The 304 path: the body wasn't re-sent, so the cached version is still the most recent thing
/// seen and overwriting it with nothing would lose what the UI shows.
#[test]
fn a_success_with_no_version_leaves_the_last_known_release_alone() {
    let mut flags = UpdateFlags {
        last_known_release: "0.3.0".to_owned(),
        last_manifest_etag: "\"m1\"".to_owned(),
        consecutive_failures: 4,
        ..UpdateFlags::default()
    };

    flags.record_success(1_717_243_200, None, None);

    assert_eq!(flags.last_known_release, "0.3.0");
    assert_eq!(flags.last_manifest_etag, "\"m1\"");
    assert_eq!(flags.consecutive_failures, 0, "a reachable server clears the backoff");
    assert_eq!(flags.last_check_unix, 1_717_243_200);
}

#[test]
fn a_success_carrying_a_version_and_etag_stores_both() {
    let mut flags = UpdateFlags {
        last_known_release: "0.3.0".to_owned(),
        last_manifest_etag: "\"m1\"".to_owned(),
        ..UpdateFlags::default()
    };

    flags.record_success(1_717_243_200, Some("0.4.0".to_owned()), Some("\"m2\"".to_owned()));

    assert_eq!(flags.last_known_release, "0.4.0");
    assert_eq!(flags.last_manifest_etag, "\"m2\"");
}

/// A skip is never cleared by a check succeeding — only by a strictly newer release, which is the
/// daily task's call and not this layer's.
#[test]
fn recording_a_check_never_touches_the_skipped_release() {
    let mut flags = UpdateFlags {
        skipped_release: "0.3.0".to_owned(),
        ..UpdateFlags::default()
    };

    flags.record_success(1, Some("0.4.0".to_owned()), None);
    flags.record_failure(2);

    assert_eq!(flags.skipped_release, "0.3.0");
}

#[test]
fn serde_round_trip_preserves_all_fields() -> TestResult {
    let original = UpdateFlags {
        auto_check_enabled: false,
        last_check_unix: 1_717_243_200,
        last_known_release: "0.2.5".to_owned(),
        skipped_release: "0.2.0".to_owned(),
        last_manifest_etag: "\"some-etag-value\"".to_owned(),
        consecutive_failures: 3,
    };
    let json = serde_json::to_string(&original)?;
    let parsed: UpdateFlags = serde_json::from_str(&json)?;
    assert_eq!(parsed.auto_check_enabled, original.auto_check_enabled);
    assert_eq!(parsed.last_check_unix, original.last_check_unix);
    assert_eq!(parsed.last_known_release, original.last_known_release);
    assert_eq!(parsed.skipped_release, original.skipped_release);
    assert_eq!(parsed.last_manifest_etag, original.last_manifest_etag);
    assert_eq!(parsed.consecutive_failures, original.consecutive_failures);
    Ok(())
}

#[test]
fn missing_fields_fall_back_to_defaults() -> TestResult {
    // Older `settings.json` files won't have any `updates.*` keys.
    // `#[serde(default)]` on the substruct must produce the same value
    // as `UpdateFlags::default()` rather than a deserialization error.
    let empty_object = "{}";
    let parsed: UpdateFlags = serde_json::from_str(empty_object)?;
    let expected = UpdateFlags::default();
    assert_eq!(parsed.auto_check_enabled, expected.auto_check_enabled);
    assert_eq!(parsed.last_check_unix, expected.last_check_unix);
    assert_eq!(parsed.consecutive_failures, expected.consecutive_failures);
    Ok(())
}
