use melodia_core::error::AppError;

use super::{CheckOutcome, classify_manifest};
use crate::services::updater::manifest::{
    LatestManifest, PlatformAsset, SUPPORTED_MANIFEST_SCHEMA,
};

const TARGET: &str = "linux-x86_64-tarball";

fn asset() -> PlatformAsset {
    PlatformAsset {
        url: "https://example.test/melodia-0.3.0-x86_64-linux.tar.gz".into(),
        signature: "untrusted comment: signature\nRWQ=\ntrusted comment: version=0.3.0\nRWQ=\n"
            .into(),
        size: 52_428_800,
    }
}

fn manifest(version: &str, schema: u32, platforms: &[&str]) -> LatestManifest {
    LatestManifest {
        manifest_schema_version: schema,
        version: version.into(),
        critical: false,
        pub_date: "2026-09-04T00:00:00Z".into(),
        notes_short: "Fixes things".into(),
        platforms: platforms.iter().map(|key| ((*key).to_owned(), asset())).collect(),
    }
}

/// A strong tag, the only kind the resume protocol keeps.
const ETAG: &str = r#""abc123""#;

/// The mechanism by which an older client refuses a manifest it cannot read. Getting it backwards
/// means either refusing every valid manifest or acting on one whose shape it is guessing at.
#[test]
fn a_schema_past_the_supported_one_is_refused_and_the_etag_kept() -> Result<(), AppError> {
    let outcome = classify_manifest(
        manifest("0.3.0", SUPPORTED_MANIFEST_SCHEMA + 1, &[TARGET]),
        Some(ETAG.to_owned()),
        "0.2.0",
        Some(TARGET),
    )?;

    let CheckOutcome::UnsupportedSchema {
        schema,
        etag: cached,
    } = outcome
    else {
        return Err(AppError::Validation(format!("expected UnsupportedSchema, got {outcome:?}")));
    };
    assert_eq!(schema, SUPPORTED_MANIFEST_SCHEMA + 1);
    // Cached on the refusing path so the JSON parse isn't re-paid every six hours.
    assert_eq!(cached.as_deref(), Some(ETAG));
    Ok(())
}

#[test]
fn the_supported_schema_itself_proceeds() -> Result<(), AppError> {
    let outcome = classify_manifest(
        manifest("0.3.0", SUPPORTED_MANIFEST_SCHEMA, &[TARGET]),
        Some(ETAG.to_owned()),
        "0.2.0",
        Some(TARGET),
    )?;
    assert!(matches!(outcome, CheckOutcome::Available { .. }), "got {outcome:?}");
    Ok(())
}

/// The gate runs ahead of the semver comparison, so a manifest this client cannot read is refused
/// rather than quietly reading as "up to date" — the two outcomes are indistinguishable to the
/// caller but only one of them tells the user to upgrade out of band.
#[test]
fn the_schema_gate_runs_before_the_semver_gate() -> Result<(), AppError> {
    let outcome = classify_manifest(
        manifest("0.1.0", SUPPORTED_MANIFEST_SCHEMA + 1, &[TARGET]),
        Some(ETAG.to_owned()),
        "0.2.0",
        Some(TARGET),
    )?;
    assert!(matches!(outcome, CheckOutcome::UnsupportedSchema { .. }), "got {outcome:?}");
    Ok(())
}

#[test]
fn a_version_that_is_not_an_upgrade_is_up_to_date() -> Result<(), AppError> {
    for candidate in ["0.2.0", "0.1.9"] {
        let outcome = classify_manifest(
            manifest(candidate, SUPPORTED_MANIFEST_SCHEMA, &[TARGET]),
            Some(ETAG.to_owned()),
            "0.2.0",
            Some(TARGET),
        )?;
        assert!(matches!(outcome, CheckOutcome::UpToDate), "{candidate}: got {outcome:?}");
    }
    Ok(())
}

/// An unpackaged host: Melodia runs, but no release asset can be named for it. Nothing to install
/// rather than a failure, and the etag is still worth keeping.
#[test]
fn a_host_with_no_target_key_has_no_asset_to_offer() -> Result<(), AppError> {
    let outcome = classify_manifest(
        manifest("0.3.0", SUPPORTED_MANIFEST_SCHEMA, &[TARGET]),
        Some(ETAG.to_owned()),
        "0.2.0",
        None,
    )?;

    let CheckOutcome::NoAssetForTarget { etag: cached } = outcome else {
        return Err(AppError::Validation(format!("expected NoAssetForTarget, got {outcome:?}")));
    };
    assert_eq!(cached.as_deref(), Some(ETAG));
    Ok(())
}

/// The v0.8.0 shape: a real upgrade, a key the client asks for, and a manifest that never listed
/// it because the filename regex stopped matching what the build renamed. An error rather than a
/// silent no-op, so it reaches the user as a failed update instead of an indefinite "up to date".
#[test]
fn a_known_key_missing_from_the_manifest_is_an_error() -> Result<(), AppError> {
    let outcome = classify_manifest(
        manifest("0.3.0", SUPPORTED_MANIFEST_SCHEMA, &["linux-x86_64-deb"]),
        Some(ETAG.to_owned()),
        "0.2.0",
        Some(TARGET),
    );

    let Err(AppError::Validation(msg)) = outcome else {
        return Err(AppError::Validation(format!("expected a Validation error, got {outcome:?}")));
    };
    assert!(msg.contains(TARGET), "the message must name the key that was missing: {msg}");
    Ok(())
}

#[test]
fn an_upgrade_resolves_the_asset_for_the_running_target() -> Result<(), AppError> {
    let mut with_two = manifest("0.3.0", SUPPORTED_MANIFEST_SCHEMA, &[TARGET]);
    with_two.platforms.insert(
        "linux-aarch64-rpm".into(),
        PlatformAsset {
            url: "https://example.test/other".into(),
            ..asset()
        },
    );

    let outcome = classify_manifest(with_two, Some(ETAG.to_owned()), "0.2.0", Some(TARGET))?;

    let CheckOutcome::Available {
        manifest,
        asset: picked,
        etag: cached,
    } = outcome
    else {
        return Err(AppError::Validation(format!("expected Available, got {outcome:?}")));
    };
    assert_eq!(manifest.version, "0.3.0");
    assert_eq!(picked.url, asset().url, "the asset must be the running target's, not the first");
    assert_eq!(cached.as_deref(), Some(ETAG));
    Ok(())
}

/// `is_upgrade` parses both sides, so a manifest carrying a version string semver can't read is a
/// hard error. Falling back to "up to date" would strand a client on a typo in the release job.
#[test]
fn an_unparseable_manifest_version_is_an_error() {
    let outcome = classify_manifest(
        manifest("v0.3.0", SUPPORTED_MANIFEST_SCHEMA, &[TARGET]),
        Some(ETAG.to_owned()),
        "0.2.0",
        Some(TARGET),
    );
    assert!(matches!(outcome, Err(AppError::Validation(_))), "got {outcome:?}");
}
