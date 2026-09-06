//! The sequencing promise this module exists to keep: verification completes before anything
//! renames onto the live binary, and a failure takes both staged files with it.
//!
//! Only the refusing path is driven, and it is the one worth driving. A passing verify needs a
//! signature from the release key, which is the seam `github_tests.rs` argues against adding —
//! and the failure is where the promise actually has teeth, since a success leaves nothing to
//! clean up and nothing to protect.

use melodia_testkit::http::{TestResponse, TestServer};
use tempfile::tempdir;

use super::staging::{InstallMethod, resolve_install_method, sidecar_meta_path, staged_path};
use super::{download_and_install_to, old_path};
use crate::services::updater::manifest::PlatformAsset;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ARTIFACT: &[u8] = b"NEW BINARY BYTES";
const LIVE: &[u8] = b"v0.2.0 LIVE BINARY";

/// Well-formed minisign output over other bytes entirely, so verification reaches the signature
/// check and fails there rather than earlier on a parse.
const WRONG_SIGNATURE: &str = include_str!("fixtures/test-data.minisig");

/// Every artifact this suite serves is refused, so the install never runs and the target is only
/// ever read. That is the assertion, not an accident of the fixture.
#[tokio::test]
async fn a_signature_that_does_not_verify_leaves_the_live_binary_untouched() -> TestResult {
    assert_eq!(
        resolve_install_method(),
        InstallMethod::AtomicSwap,
        "test setup: a cargo-built binary is owned by no package, so the swap is the path here"
    );

    let server = TestServer::start(|_| TestResponse::ok(ARTIFACT))?;
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    std::fs::write(&target, LIVE)?;

    let asset = PlatformAsset {
        url: format!("{}/melodia-0.3.0-x86_64-linux.tar.gz", server.base_url()),
        signature: WRONG_SIGNATURE.to_owned(),
        size: ARTIFACT.len() as u64,
    };

    let outcome =
        download_and_install_to(&reqwest::Client::new(), &asset, "0.3.0", target.clone(), |_| {})
            .await;

    let Err(melodia_core::error::AppError::Validation(msg)) = outcome else {
        return Err(format!("expected a signature refusal, got {outcome:?}").into());
    };
    assert!(msg.contains("signature verification failed"), "{msg}");

    assert_eq!(std::fs::read(&target)?, LIVE, "the live binary must not have been replaced");
    assert!(!old_path(&target).exists(), "and no rollback snapshot should exist to replace it");

    let staged = staged_path(&target);
    assert!(!staged.exists(), "the unverified bytes must be removed");
    assert!(
        !sidecar_meta_path(&staged).exists(),
        "and the sidecar with them, or the next attempt resumes against a fingerprint that \
         vouches for bytes nothing verified"
    );

    assert!(!server.requests().is_empty(), "test setup: the artifact must have been fetched");
    Ok(())
}

/// The download aborts before the verify, so the target is untouched for a second reason. Worth
/// its own case because the cleanup here is the download's, not the install's, and the two have
/// gone out of step before.
#[tokio::test]
async fn an_oversized_response_never_reaches_the_install() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok(vec![b'x'; 8192]))?;
    let dir = tempdir()?;
    let target = dir.path().join("Melodia");
    std::fs::write(&target, LIVE)?;

    let asset = PlatformAsset {
        url: format!("{}/melodia-0.3.0-x86_64-linux.tar.gz", server.base_url()),
        signature: WRONG_SIGNATURE.to_owned(),
        size: ARTIFACT.len() as u64,
    };

    let outcome =
        download_and_install_to(&reqwest::Client::new(), &asset, "0.3.0", target.clone(), |_| {})
            .await;

    assert!(outcome.is_err(), "8192 bytes against a declared 16 must abort");
    assert_eq!(std::fs::read(&target)?, LIVE);
    assert!(!staged_path(&target).exists());
    Ok(())
}
