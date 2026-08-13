use std::io::Cursor;

use minisign_verify::PublicKey;

use crate::services::updater::minisign::{
    MinisignError, embedded_pubkey, parse_trusted_comment_version, verify_manifest_bytes,
    verify_stream,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TEST_PUBKEY_FILE: &str = include_str!("fixtures/test-pubkey.b64");
const TEST_DATA: &[u8] = include_bytes!("fixtures/test-data.bin");
const TEST_SIG: &str = include_str!("fixtures/test-data.minisig");
const TEST_NOHASH_DATA: &[u8] = include_bytes!("fixtures/test-data-nohash.bin");
const TEST_NOHASH_SIG: &str = include_str!("fixtures/test-data-nohash.minisig");

// Fixture pair signed with `-t "version=0.42.0 target=…"` for the
// trusted-comment cross-check tests. Regenerate via
// `bash src/services/updater/tests/fixtures/regen-versioned.sh`.
const TEST_PUBKEY_VERSIONED: &str = include_str!("fixtures/test-pubkey-versioned.b64");
const TEST_DATA_VERSIONED: &[u8] = include_bytes!("fixtures/test-data-versioned.bin");
const TEST_SIG_VERSIONED: &str = include_str!("fixtures/test-data-versioned.minisig");

// Fixture pair signed with `-t "version=0.2.0 manifest=true"` for the
// verify_manifest_bytes tests. Regenerate via
// `bash src/services/updater/tests/fixtures/regen-manifest.sh`.
const TEST_PUBKEY_MANIFEST: &str = include_str!("fixtures/test-pubkey-manifest.b64");
const TEST_MANIFEST: &[u8] = include_bytes!("fixtures/test-manifest.json");
const TEST_SIG_MANIFEST: &str = include_str!("fixtures/test-manifest.minisig");

#[test]
fn embedded_pubkey_parses_as_full_file() -> TestResult {
    // Regression for the #1 trap: `PublicKey::from_base64()` rejects
    // the full multi-line file. The production pubkey is the full file
    // — make sure `embedded_pubkey()` (which uses `PublicKey::decode`)
    // actually accepts the format `minisign -G` produces.
    embedded_pubkey()?;
    Ok(())
}

#[test]
fn fixture_pubkey_parses_as_full_file() {
    assert!(
        PublicKey::decode(TEST_PUBKEY_FILE).is_ok(),
        "PublicKey::decode failed on fixture file",
    );
}

#[test]
fn valid_prehashed_signature_verifies() -> TestResult {
    // End-to-end happy path: matching key, prehashed sig, untouched
    // data. This is the regression net for the streaming path itself
    // — if `update()` ever fails to feed bytes into the verifier the
    // hash mismatch would surface here.
    let pk = PublicKey::decode(TEST_PUBKEY_FILE)?;
    // Existing legacy fixture has no `version=` in its trusted comment;
    // pass `None` to opt out of the cross-check. The new
    // `versioned_signature_verifies_when_version_matches` test covers
    // the Some path.
    verify_stream(Cursor::new(TEST_DATA), TEST_SIG, &pk, None)?;
    Ok(())
}

#[test]
fn truncated_signature_is_parse_error() -> TestResult {
    let pk = PublicKey::decode(TEST_PUBKEY_FILE)?;
    let truncated = TEST_SIG.lines().take(2).collect::<Vec<_>>().join("\n");
    let result = verify_stream(Cursor::new(TEST_DATA), &truncated, &pk, None);
    assert!(
        matches!(result, Err(MinisignError::SignatureDecode(_))),
        "expected SignatureDecode error, got {result:?}",
    );
    Ok(())
}

#[test]
fn non_prehashed_signature_is_rejected() -> TestResult {
    // The fixture signature was produced WITHOUT `-H` (trusted comment
    // ends in `hashed`, not `prehashed`). `verify_stream` must reject
    // before processing any data, otherwise a release accidentally
    // signed without `-H` would silently break the streaming path.
    let pk = PublicKey::decode(TEST_PUBKEY_FILE)?;
    let result = verify_stream(Cursor::new(TEST_NOHASH_DATA), TEST_NOHASH_SIG, &pk, None);
    let Err(MinisignError::StreamSetup(msg)) = result else {
        return Err(format!("expected StreamSetup error, got {result:?}").into());
    };
    assert!(msg.contains("prehashed"), "rejection message should mention prehashed: {msg}");
    Ok(())
}

#[test]
fn modified_data_fails_verification() -> TestResult {
    // Flip one byte of the data. The streaming verifier must consume
    // the bytes and notice the hash mismatch at `finalize()`.
    let pk = PublicKey::decode(TEST_PUBKEY_FILE)?;
    let mut tampered = TEST_DATA.to_vec();
    if let Some(b) = tampered.first_mut() {
        *b ^= 0xff;
    }
    let result = verify_stream(Cursor::new(tampered), TEST_SIG, &pk, None);
    assert!(
        matches!(result, Err(MinisignError::Verify(_))),
        "modified data must surface as Verify error, got {result:?}",
    );
    Ok(())
}

#[test]
fn parse_trusted_comment_version_extracts_token() {
    // Real shape we sign with: `version=… target=… file=…`.
    let comment = "version=0.42.0 target=linux-x86_64-tarball file=foo.tar.gz";
    assert_eq!(parse_trusted_comment_version(comment), Some("0.42.0"));

    // Order-insensitive — `version=` need not be first.
    let comment = "target=linux file=foo version=1.2.3";
    assert_eq!(parse_trusted_comment_version(comment), Some("1.2.3"));

    // Missing field surfaces as None.
    let comment = "timestamp:1717243200	file:foo.bin	prehashed";
    assert_eq!(parse_trusted_comment_version(comment), None);

    // Empty value is technically extractable; the caller's equality
    // check will reject. Test for parser symmetry, not policy.
    assert_eq!(parse_trusted_comment_version("version= foo"), Some(""));
}

#[test]
fn versioned_signature_verifies_when_version_matches() -> TestResult {
    // Happy path for the trusted-comment cross-check: the fixture's
    // trusted comment carries `version=0.42.0` and the caller asserts
    // the same.
    let pk = PublicKey::decode(TEST_PUBKEY_VERSIONED)?;
    verify_stream(Cursor::new(TEST_DATA_VERSIONED), TEST_SIG_VERSIONED, &pk, Some("0.42.0"))?;
    Ok(())
}

#[test]
fn versioned_signature_rejected_when_version_differs() -> TestResult {
    // CDN-swap defence: the cryptographic signature would pass (this
    // IS a legitimately-signed-by-us artifact at version 0.42.0), but
    // the caller is trying to install version 9.9.9 — the cross-check
    // must catch the mismatch.
    let pk = PublicKey::decode(TEST_PUBKEY_VERSIONED)?;
    let result =
        verify_stream(Cursor::new(TEST_DATA_VERSIONED), TEST_SIG_VERSIONED, &pk, Some("9.9.9"));
    let Err(MinisignError::VersionMismatch(msg)) = result else {
        return Err(format!("expected VersionMismatch error, got {result:?}").into());
    };
    assert!(msg.contains("9.9.9"), "msg should name expected: {msg}");
    assert!(msg.contains("0.42.0"), "msg should name observed: {msg}");
    Ok(())
}

#[test]
fn manifest_signature_verifies_with_manifest_tag() -> TestResult {
    // Happy path for verify_manifest_bytes — the fixture's trusted
    // comment carries `manifest=true` and the caller asserts the same.
    let pk = PublicKey::decode(TEST_PUBKEY_MANIFEST)?;
    verify_manifest_bytes(TEST_MANIFEST, TEST_SIG_MANIFEST, &pk, None, Some("true"))?;
    Ok(())
}

#[test]
fn manifest_signature_rejected_when_manifest_tag_missing() -> TestResult {
    // The versioned artifact fixture's trusted comment carries
    // `version=0.42.0 target=… file=…` — no `manifest=` token. Cross-
    // pasting it into the manifest verification slot must fail, even
    // though the cryptographic signature itself is valid (same key
    // family, ish — for this test the key id won't match but the
    // earlier tests assert the missing-tag path on the matching key).
    // Use a versioned signature against the manifest pubkey: signature
    // decode succeeds (it's structurally valid) but the manifest-tag
    // assertion is what we want to test, so we feed the manifest fixture
    // signed by the manifest key but assert manifest_tag=wrong.
    let pk = PublicKey::decode(TEST_PUBKEY_MANIFEST)?;
    let result = verify_manifest_bytes(TEST_MANIFEST, TEST_SIG_MANIFEST, &pk, None, Some("wrong"));
    let Err(MinisignError::VersionMismatch(msg)) = result else {
        return Err(format!("expected VersionMismatch error, got {result:?}").into());
    };
    assert!(
        msg.contains("manifest-tag mismatch") && msg.contains("wrong"),
        "rejection message should explain the mismatch: {msg}",
    );
    Ok(())
}

#[test]
fn manifest_signature_rejects_artifact_sig_cross_paste() -> TestResult {
    // An artifact .minisig (from the versioned fixture) lacks the
    // `manifest=` token entirely. Feeding it into verify_manifest_bytes
    // with `expected_manifest_tag=Some("true")` must surface the
    // "no `manifest=` field" diagnostic — this is the domain-
    // separation check that prevents an attacker from cross-pasting
    // a legitimately-signed artifact .minisig into the manifest slot.
    let pk = PublicKey::decode(TEST_PUBKEY_VERSIONED)?;
    let result =
        verify_manifest_bytes(TEST_DATA_VERSIONED, TEST_SIG_VERSIONED, &pk, None, Some("true"));
    let Err(MinisignError::VersionMismatch(msg)) = result else {
        return Err(format!("expected VersionMismatch error, got {result:?}").into());
    };
    assert!(
        msg.contains("no `manifest=` field"),
        "rejection message should explain the missing tag: {msg}",
    );
    Ok(())
}

#[test]
fn manifest_signature_rejects_tampered_body() -> TestResult {
    let pk = PublicKey::decode(TEST_PUBKEY_MANIFEST)?;
    let mut tampered = TEST_MANIFEST.to_vec();
    if let Some(b) = tampered.first_mut() {
        *b ^= 0xff;
    }
    let result = verify_manifest_bytes(&tampered, TEST_SIG_MANIFEST, &pk, None, Some("true"));
    assert!(
        matches!(result, Err(MinisignError::Verify(_))),
        "tampered manifest body must surface as Verify error, got {result:?}",
    );
    Ok(())
}

#[test]
fn legacy_signature_rejected_when_version_required() -> TestResult {
    // Old releases (and the original test fixture) were signed without
    // `-t "version=…"`. Production must enforce — a release that
    // forgets the `-t` flag in CI must fail verification rather than
    // silently passing the cross-check.
    let pk = PublicKey::decode(TEST_PUBKEY_FILE)?;
    let result = verify_stream(Cursor::new(TEST_DATA), TEST_SIG, &pk, Some("1.0.0"));
    let Err(MinisignError::VersionMismatch(msg)) = result else {
        return Err(format!("expected VersionMismatch error, got {result:?}").into());
    };
    assert!(msg.contains("no `version=` field"), "msg should explain the missing field: {msg}");
    Ok(())
}
