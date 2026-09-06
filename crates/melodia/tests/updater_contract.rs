//! The four updater couplings that span trees, where neither half can see the other break.
//!
//! Each one fails silently in CI and loudly in an install: a renamed trusted-comment token makes
//! every client reject the release as `VersionMismatch`, a platform key the manifest stops naming
//! leaves that format's users on "no update available" indefinitely (v0.8.0's deb slot, which is
//! why the script carries a comment about it), a schema default that drifts from the client's
//! constant makes every client refuse the manifest, and a swapped public key is an unintended
//! rotation. None of them has an anchor file, because the argument is the agreement itself.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use melodia_app::services::updater::manifest::SUPPORTED_MANIFEST_SCHEMA;
use melodia_testkit::{REPO_ROOT, strip_line_comments};

/// One artifact-signing call plus the two that sign a manifest. An equality rather than a floor:
/// a fourth signing workflow added without the fields is exactly the regression, and a floor
/// would wave it through.
const SIGNING_INVOCATIONS: usize = 3;

/// Ten from `format_key`, twelve from the script. Equalities so that adding a platform has to be
/// done on both sides at once rather than on whichever one the change started in.
const CLIENT_KEYS: usize = 10;
const MANIFEST_KEYS: usize = 12;

fn read(path: &Path) -> String {
    let src = fs::read_to_string(path).unwrap_or_default();
    assert!(!src.is_empty(), "unreadable or empty: {}", path.display());
    src
}

/// Every workflow under `.github/workflows/`, so a new one signing without the fields is caught
/// by being walked rather than by being remembered.
fn workflows() -> Vec<(String, String)> {
    let dir = Path::new(REPO_ROOT).join(".github/workflows");
    let listing = fs::read_dir(&dir);
    assert!(listing.is_ok(), "`{}` would not list", dir.display());

    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    for entry in listing.into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("yml") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        match fs::read_to_string(&path) {
            Ok(text) => found.push((name, text)),
            Err(_) => unreadable.push(name),
        }
    }

    // Counted rather than skipped: a workflow that fails to read is one this walk would otherwise
    // clear without looking at.
    assert!(unreadable.is_empty(), "unreadable workflows: {unreadable:?}");
    assert!(found.len() >= SIGNING_INVOCATIONS, "only {} workflows found", found.len());
    found.sort();
    found
}

fn trusted_comments(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|line| line.trim().strip_prefix("trusted-comment:"))
        .map(|value| value.trim().to_owned())
        .collect()
}

/// Every double-quoted literal shaped like a `latest.json` platform key. Both halves of the
/// comparison spell their keys as literals in a table, so reading them back is the only way to
/// ask whether the two tables still agree.
fn platform_keys(src: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut rest = src;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let literal = &after[..close];
        if literal.starts_with("linux-") || literal.starts_with("windows-") {
            keys.insert(literal.to_owned());
        }
        rest = &after[close + 1..];
    }
    keys
}

/// `minisign.rs` parses `version=` out of every signature and `manifest=` out of the manifest's,
/// and refuses the install when either is missing. The tokens are produced three workflows away
/// and nothing else asks whether the two vocabularies still match.
#[test]
fn every_signature_carries_the_trusted_comment_fields_the_client_parses() {
    let mut comments = Vec::new();
    let mut invocations = 0;
    for (name, src) in workflows() {
        invocations += src.matches("uses: ./.github/actions/minisign-sign").count();
        comments.extend(trusted_comments(&src).into_iter().map(|value| (name.clone(), value)));
    }

    assert_eq!(
        invocations, SIGNING_INVOCATIONS,
        "the signing action is used {invocations} times; a new caller owes the same fields"
    );
    assert_eq!(
        comments.len(),
        SIGNING_INVOCATIONS,
        "every minisign-sign call must pass a trusted comment, got {comments:?}"
    );

    for (name, comment) in &comments {
        assert!(
            comment.contains("version="),
            "{name}: the client asserts version= against the manifest it is installing: {comment}"
        );
    }

    let manifests: Vec<&String> =
        comments.iter().filter(|(_, c)| c.contains("manifest=true")).map(|(n, _)| n).collect();
    let artifacts: Vec<&(String, String)> =
        comments.iter().filter(|(_, c)| c.contains("file={file}")).collect();

    // `manifest=true` is the domain separation that stops an artifact signature being swapped into
    // the manifest's slot; both the draft-time build and the post-publish refresh owe it.
    assert_eq!(manifests.len(), 2, "expected two manifest signatures, got {manifests:?}");
    assert_eq!(artifacts.len(), 1, "expected one artifact signature, got {artifacts:?}");
    let (name, artifact) = artifacts[0];
    assert!(
        artifact.contains("target="),
        "{name}: the artifact comment names the target it was built for: {artifact}"
    );
}

/// The `-t` flag itself lives in the composite action all three callers delegate to, so the
/// fields above reach a signature only while this step still passes them. `-SH` is the other half:
/// a signature that isn't prehashed can't be verified as the artifact streams, and the client
/// rejects it outright.
#[test]
fn the_signing_step_still_prehashes_and_writes_the_comment() {
    let path = Path::new(REPO_ROOT).join(".github/actions/minisign-sign/action.yml");
    let src = read(&path);

    assert!(src.contains("minisign -SHm"), "signing must stay prehashed");
    assert!(src.contains("-t \"${COMMENT"), "the trusted comment must still be signed");
    assert!(
        src.contains("${COMMENT//\\{file\\}/"),
        "the per-artifact comment depends on {{file}} being substituted"
    );
}

/// `format_key` produces the keys a client asks for; `build-latest-json.py` decides which ones the
/// manifest carries. A key on the client's side alone reaches the user as a failed update naming
/// a platform asset that was never published.
#[test]
fn every_platform_key_the_client_asks_for_is_one_the_manifest_can_carry() {
    let client = platform_keys(&strip_line_comments(&read(
        &Path::new(REPO_ROOT)
            .join("crates/melodia-platform/src/services/platform/install_kind/target.rs"),
    )));
    let manifest = platform_keys(&read(&Path::new(REPO_ROOT).join("scripts/build-latest-json.py")));

    assert_eq!(client.len(), CLIENT_KEYS, "client keys drifted: {client:?}");
    assert_eq!(manifest.len(), MANIFEST_KEYS, "manifest keys drifted: {manifest:?}");

    let unpublishable: Vec<&String> = client.difference(&manifest).collect();
    assert!(
        unpublishable.is_empty(),
        "{unpublishable:?} can be asked for but never appears in a manifest — the filename \
         patterns are downstream of the packaging steps and must move with them"
    );
}

/// `check.rs` refuses any manifest declaring a schema above the constant, so the value CI writes
/// and the value the client understands are one protocol. The constant used to be pinned against
/// itself with a comment telling the reader to keep the script in step by hand.
#[test]
fn the_manifest_schema_the_client_supports_is_the_one_ci_writes() {
    let src = read(&Path::new(REPO_ROOT).join("scripts/build-latest-json.py"));
    let flag = src.find("\"--manifest-schema-version\"");
    assert!(flag.is_some(), "the script no longer declares --manifest-schema-version");

    let written = flag
        .and_then(|at| src[at..].find("default=").map(|d| at + d + "default=".len()))
        .map(|at| src[at..].chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|digits| digits.parse::<u32>().ok());

    assert_eq!(
        written,
        Some(SUPPORTED_MANIFEST_SCHEMA),
        "the script's default schema and the client's supported schema must move together"
    );
}

/// Rotation is documented as one-way and disruptive; an accidental rotation is the same event
/// without the intent, and a fixture key committed over this file would still parse. Only the key
/// material is pinned — the untrusted comment above it is free text minisign never verifies.
#[test]
fn the_embedded_pubkey_is_still_the_release_key() {
    let path = Path::new(REPO_ROOT).join("assets/updater-pubkey.b64");
    let src = read(&path);

    let key = src.lines().map(str::trim).filter(|line| !line.is_empty()).nth(1);
    assert_eq!(
        key,
        Some("RWS7ypNIpu/lrcx22niqkHnyQrT5oAnCORcg1+rmzTRxasPaqlKT7BoU"),
        "the updater's public key changed; every install already in the field verifies against \
         the old one, so this is only correct as a deliberate rotation"
    );
}
