use crate::services::updater::manifest::{
    LatestManifest, PlatformAsset, SUPPORTED_MANIFEST_SCHEMA,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SAMPLE_JSON: &str = r#"{
  "version": "0.2.0",
  "pub_date": "2026-06-01T12:00:00Z",
  "notes_short": "Bug fixes and stability improvements.",
  "platforms": {
    "linux-x86_64-appimage": {
      "url": "https://example.test/melodia-0.2.0-x86_64.AppImage",
      "signature": "untrusted comment: signature from minisign secret key\nRUQf...\ntrusted comment: timestamp:1717243200\tfile:melodia-0.2.0-x86_64.AppImage\tprehashed\nQtKM...",
      "size": 52428800
    },
    "linux-x86_64-tarball": {
      "url": "https://example.test/melodia-0.2.0-x86_64-linux.tar.gz",
      "signature": "untrusted comment: signature from minisign secret key\nRUQg...\ntrusted comment: timestamp:1717243200\tfile:melodia-0.2.0-x86_64-linux.tar.gz\tprehashed\nQtKN...",
      "size": 18874368
    }
  }
}"#;

#[test]
fn round_trip_preserves_multiline_signature() -> TestResult {
    let parsed: LatestManifest = serde_json::from_str(SAMPLE_JSON)?;
    assert_eq!(parsed.version, "0.2.0");
    assert_eq!(parsed.notes_short, "Bug fixes and stability improvements.");
    let appimage = parsed
        .platforms
        .get("linux-x86_64-appimage")
        .ok_or("missing linux-x86_64-appimage asset")?;
    assert_eq!(appimage.size, 52_428_800);
    // The multi-line signature must round-trip the embedded `\n`s
    // through serde — without it the client-side
    // `minisign::Signature::decode` would see a single garbled line and
    // refuse to parse.
    assert!(
        appimage.signature.contains("\nRUQf"),
        "signature should preserve newlines: {:?}",
        appimage.signature
    );
    assert!(
        appimage.signature.contains("\ntrusted comment:"),
        "signature should preserve trusted-comment line break"
    );
    assert!(
        appimage.signature.contains("\nQtKM"),
        "signature should preserve global-signature line break"
    );

    // Re-serialize and re-parse — make sure the trip back through serde
    // doesn't drop the newlines either.
    let json = serde_json::to_string(&parsed)?;
    let reparsed: LatestManifest = serde_json::from_str(&json)?;
    let reparsed_appimage = reparsed
        .platforms
        .get("linux-x86_64-appimage")
        .ok_or("missing linux-x86_64-appimage after re-parse")?;
    assert_eq!(reparsed_appimage.signature, appimage.signature);
    Ok(())
}

#[test]
fn missing_platforms_key_is_parse_error() {
    let bad = r#"{ "version": "0.2.0" }"#;
    let result: serde_json::Result<LatestManifest> = serde_json::from_str(bad);
    assert!(result.is_err(), "manifest without platforms must fail to parse");
}

#[test]
fn back_compat_parse_omits_new_fields() -> TestResult {
    // A manifest produced before manifest_schema_version + critical
    // existed must still parse — the fields default to 1 / false. This
    // protects the in-flight pre-versioned manifest the client consumes
    // exactly once on the upgrade boundary.
    let pre_versioned = r#"{
      "version": "0.1.5",
      "platforms": {
        "linux-x86_64-tarball": {
          "url": "https://example.test/melodia-0.1.5-x86_64-linux.tar.gz",
          "signature": "sig",
          "size": 100
        }
      }
    }"#;
    let parsed: LatestManifest = serde_json::from_str(pre_versioned)?;
    assert_eq!(parsed.manifest_schema_version, 1);
    assert!(
        !parsed.critical,
        "critical must default to false when the field is absent"
    );
    Ok(())
}

#[test]
fn round_trip_preserves_new_fields() -> TestResult {
    let manifest = LatestManifest {
        manifest_schema_version: 1,
        version: "0.2.0".to_owned(),
        critical: true,
        pub_date: "2026-06-01T12:00:00Z".to_owned(),
        notes_short: "Security fix.".to_owned(),
        platforms: std::collections::HashMap::new(),
    };
    let json = serde_json::to_string(&manifest)?;
    let reparsed: LatestManifest = serde_json::from_str(&json)?;
    assert_eq!(reparsed.manifest_schema_version, 1);
    assert!(reparsed.critical);
    assert_eq!(reparsed.version, "0.2.0");
    Ok(())
}

#[test]
fn supported_schema_constant_is_one() {
    // Tripwire: bumping SUPPORTED_MANIFEST_SCHEMA is a coordinated
    // protocol change. If you bump it, also update build-latest-json.py's
    // --manifest-schema-version default + the CI invocation that produces
    // the live latest.json.
    assert_eq!(SUPPORTED_MANIFEST_SCHEMA, 1);
}

#[test]
fn unknown_platform_keys_pass_through() -> TestResult {
    // Adding a new platform key (e.g. macOS later) must not break
    // existing clients that don't know about it. They just won't find
    // their key via `current_target_key()`. Round-trip an unknown key
    // and assert it's preserved.
    let json = r#"{
      "version": "0.3.0",
      "platforms": {
        "macos-aarch64-dmg": {
          "url": "https://example.test/melodia-0.3.0.dmg",
          "signature": "sig",
          "size": 1
        }
      }
    }"#;
    let parsed: LatestManifest = serde_json::from_str(json)?;
    let asset: &PlatformAsset = parsed
        .platforms
        .get("macos-aarch64-dmg")
        .ok_or("missing macos-aarch64-dmg asset")?;
    assert_eq!(asset.url, "https://example.test/melodia-0.3.0.dmg");
    Ok(())
}
