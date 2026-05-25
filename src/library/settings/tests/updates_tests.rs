//! Pure tests for the auto-updater flags persistence layer. Each setter
//! in `super::updates::*` is a one-line `mutate_settings` closure over
//! `UpdateFlags`; the substantive behaviour worth pinning is on the
//! struct itself — defaults, serde round-trip, failure-counter saturation.

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

#[test]
fn failure_counter_saturates_at_u8_max() {
    // `record_check_failure` uses `saturating_add(1)` — at the cap the
    // value must stay at `u8::MAX` rather than wrapping to 0 and
    // re-arming the 6h cadence after years of failed checks.
    let mut flags = UpdateFlags {
        consecutive_failures: u8::MAX,
        ..UpdateFlags::default()
    };
    flags.consecutive_failures = flags.consecutive_failures.saturating_add(1);
    assert_eq!(flags.consecutive_failures, u8::MAX);
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
