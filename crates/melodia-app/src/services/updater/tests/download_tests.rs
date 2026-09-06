use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use melodia_testkit::http::{TestResponse, TestServer};
use tempfile::TempDir;

use super::{ResumeState, capture_strong_etag, download_to_file, exceeds_size_bound, plan_resume};
use crate::services::updater::install::staging::{
    StagedMeta, sidecar_meta_path, write_staged_meta,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const VERSION: &str = "0.3.0";
const ARTIFACT: &[u8] = b"0123456789";
const SIZE: u64 = ARTIFACT.len() as u64;
const ETAG: &str = "\"v3\"";

#[test]
fn size_bound_allows_exact_declared_size() {
    // A response exactly matching the manifest's `size` is the happy
    // path and must not trip the bound.
    assert!(!exceeds_size_bound(80 * 1024 * 1024, 80 * 1024 * 1024));
}

#[test]
fn size_bound_allows_small_overshoot_within_slack() {
    // 3 % over — within the 5 % slack for chunked-transfer accounting
    // differences. Must not trip.
    let expected = 100_000_000;
    let downloaded = expected + (expected * 3 / 100);
    assert!(!exceeds_size_bound(downloaded, expected));
}

#[test]
fn size_bound_rejects_overshoot_past_slack() {
    // 6 % over — past the 5 % slack. Must trip.
    let expected = 100_000_000;
    let downloaded = expected + (expected * 6 / 100);
    assert!(exceeds_size_bound(downloaded, expected));
}

#[test]
fn size_bound_rejects_gigantic_overshoot() {
    // Compromised-manifest scenario: 80 MiB declared, 10 GiB served.
    // Must trip at the very first 80 MiB + slack worth of bytes.
    let expected = 80 * 1024 * 1024;
    let downloaded = expected + (expected * 6 / 100);
    assert!(exceeds_size_bound(downloaded, expected));
    // And of course the full 10 GiB trips it many times over.
    assert!(exceeds_size_bound(10u64 * 1024 * 1024 * 1024, expected));
}

#[test]
fn size_bound_handles_zero_declared() {
    // A manifest reporting `size: 0` is malformed, but the bound check
    // must not panic. Anything > 0 must trip immediately.
    assert!(exceeds_size_bound(1, 0));
    assert!(!exceeds_size_bound(0, 0));
}

#[test]
fn size_bound_saturates_on_overflow() {
    // expected_size * 105 would wrap on u64::MAX. The saturating mul
    // pins the bound at u64::MAX / 100, which is still larger than
    // any realistic downloaded value, so the check biases toward
    // "not exceeded" (i.e. we don't accidentally reject a sane
    // download because of a bogus huge manifest size). The opposite
    // bias would also be defensible; the choice here matches the
    // "always saturating-add" pattern used elsewhere.
    assert!(!exceeds_size_bound(1_000_000_000, u64::MAX));
}

#[test]
fn plan_resume_fresh_for_missing_file() {
    assert_eq!(plan_resume(0, 100), ResumeState::Fresh);
}

#[test]
fn plan_resume_skip_for_fully_matching_file() {
    // The "retention on failure" hot path: a previously-verified
    // .rpm/.deb sits on disk at the expected size. Re-downloading
    // would waste bandwidth; verify will catch any corruption.
    assert_eq!(plan_resume(100, 100), ResumeState::Skip);
    assert_eq!(plan_resume(80 * 1024 * 1024, 80 * 1024 * 1024), ResumeState::Skip);
}

#[test]
fn plan_resume_resume_for_partial_file() {
    assert_eq!(plan_resume(60, 100), ResumeState::Resume(60));
    assert_eq!(plan_resume(1, 100), ResumeState::Resume(1));
}

#[test]
fn plan_resume_fresh_for_oversized_existing() {
    // Existing > expected means leftover from a different release,
    // or a downward manifest-size shift. Safest is to discard.
    assert_eq!(plan_resume(200, 100), ResumeState::Fresh);
}

#[test]
fn plan_resume_fresh_when_manifest_size_is_zero() {
    // Malformed manifest. Force fresh so the C2 bound check at the
    // streaming layer catches whatever the server actually serves.
    assert_eq!(plan_resume(0, 0), ResumeState::Fresh);
    assert_eq!(plan_resume(100, 0), ResumeState::Fresh);
}

/// RFC 9110 §13.1.5 forbids `If-Range` with a weak entity-tag, and
/// §8.8.3.2 strong comparison would make a server silently force a full
/// re-download on every resume if we sent one. `capture_strong_etag`
/// drops weak tags at the HTTP boundary so they never reach the sidecar.
///
/// Regression guard against accidental removal of the filter — without
/// it, a future origin switch to a weak-ETag CDN would defeat resume
/// without any visible symptom in CI.
#[test]
fn capture_strong_etag_drops_weak_tags_and_keeps_strong() {
    // Weak ETags (`W/"..."`) — must be dropped.
    assert_eq!(capture_strong_etag(Some(r#"W/"abc123""#)), None);
    assert_eq!(capture_strong_etag(Some(r#"W/"""#)), None);

    // Strong ETags — must pass through unchanged. Sample shapes pulled
    // from real Azure-Blob / S3 / nginx origins.
    let azure_blob = r#""0x8DBF736055F8CFD""#;
    assert_eq!(capture_strong_etag(Some(azure_blob)), Some(azure_blob.to_owned()));
    let s3_md5 = r#""d41d8cd98f00b204e9800998ecf8427e""#;
    assert_eq!(capture_strong_etag(Some(s3_md5)), Some(s3_md5.to_owned()));

    // Missing header — `None` passes straight through.
    assert_eq!(capture_strong_etag(None), None);
}

// ---------------------------------------------------------------------------
// `download_to_file` against a canned server. The helpers above are the pieces;
// these are the wiring, where the interesting failures live.
// ---------------------------------------------------------------------------

/// Bytes plus the sidecar that vouches for them. Without a matching sidecar the download
/// discards the partial before it looks at its size, so seeding one is what puts these tests on
/// the resume path at all.
fn seed_partial(
    dir: &Path,
    bytes: &[u8],
    url: &str,
    etag: Option<&str>,
) -> std::io::Result<PathBuf> {
    let dest = dir.join("Melodia.new");
    std::fs::write(&dest, bytes)?;
    let meta = StagedMeta {
        version: VERSION.to_owned(),
        size: SIZE,
        asset_url: url.to_owned(),
        etag: etag.map(str::to_owned),
    };
    let _ = write_staged_meta(&sidecar_meta_path(&dest), &meta);
    Ok(dest)
}

/// The percentages the download reported, so a test can assert the bar's shape rather than only
/// where it ended up.
#[derive(Clone, Default)]
struct ProgressLog(Arc<Mutex<Vec<u8>>>);

impl ProgressLog {
    fn sink(&self) -> impl Fn(u8) + Send + Sync + use<> {
        let seen = Arc::clone(&self.0);
        move |pct| seen.lock().unwrap_or_else(PoisonError::into_inner).push(pct)
    }

    fn seen(&self) -> Vec<u8> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn staging() -> std::io::Result<TempDir> {
    tempfile::tempdir()
}

/// A 206 continues where the partial left off. Appending is the whole point of the resume
/// protocol; truncating first would make every resumed download a fresh one.
#[tokio::test]
async fn a_partial_content_response_appends_from_the_offset() -> TestResult {
    let server = TestServer::start(|req| match req.header("range") {
        Some("bytes=4-") => {
            TestResponse::status(206).header("Content-Range", "bytes 4-9/10").body(&ARTIFACT[4..])
        }
        _ => TestResponse::status(500),
    })?;

    let dir = staging()?;
    let url = format!("{}/melodia.tar.gz", server.base_url());
    let dest = seed_partial(dir.path(), &ARTIFACT[..4], &url, Some(ETAG))?;

    let log = ProgressLog::default();
    download_to_file(&reqwest::Client::new(), &url, VERSION, SIZE, &dest, &log.sink()).await?;

    assert_eq!(std::fs::read(&dest)?, ARTIFACT, "the resumed bytes must complete the artifact");

    let sent = server.requests();
    assert_eq!(sent.len(), 1, "expected exactly one GET, got {sent:?}");
    assert_eq!(sent[0].header("range"), Some("bytes=4-"));
    // RFC 9110 §13.1.5: the stored strong tag goes back as If-Range so the server can refuse to
    // splice bytes from a re-uploaded artifact onto the ones already on disk.
    assert_eq!(sent[0].header("if-range"), Some(ETAG));
    Ok(())
}

/// The `--clobber` hole `If-Range` exists to close: the release was re-pushed mid-download, the
/// server declines the range and sends the whole thing. Concatenating would produce a file that
/// is neither release and fails verification for a reason nobody could read.
#[tokio::test]
async fn a_full_response_to_a_range_request_restarts_from_zero() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok(ARTIFACT).header("ETag", "\"v4\""))?;

    let dir = staging()?;
    let url = format!("{}/melodia.tar.gz", server.base_url());
    let dest = seed_partial(dir.path(), &ARTIFACT[..4], &url, Some(ETAG))?;

    let log = ProgressLog::default();
    download_to_file(&reqwest::Client::new(), &url, VERSION, SIZE, &dest, &log.sink()).await?;

    let written = std::fs::read(&dest)?;
    assert_eq!(written, ARTIFACT, "a 200 must replace the partial, not extend it");
    assert_eq!(written.len(), ARTIFACT.len(), "concatenation would leave {} bytes", written.len());
    Ok(())
}

/// Retention on failure is for bytes that had a chance to verify. These never did, so the abort
/// takes them with it rather than leaving a runaway transfer's head on the user's disk.
#[tokio::test]
async fn the_size_bound_abort_removes_the_partial_file() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok(vec![b'x'; 4096]))?;

    let dir = staging()?;
    let url = format!("{}/melodia.tar.gz", server.base_url());
    let dest = dir.path().join("Melodia.new");

    let log = ProgressLog::default();
    let outcome =
        download_to_file(&reqwest::Client::new(), &url, VERSION, SIZE, &dest, &log.sink()).await;

    assert!(outcome.is_err(), "4096 bytes against a declared 10 must abort");
    assert!(!dest.exists(), "the partial must not survive a size-bound abort");
    Ok(())
}

/// The retention hot path: a verified `.rpm` whose install was cancelled is still on disk at the
/// manifest's size. Re-fetching it would spend the user's bandwidth to arrive at the same bytes.
#[tokio::test]
async fn a_complete_staged_file_is_skipped_without_a_request() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok(ARTIFACT))?;

    let dir = staging()?;
    let url = format!("{}/melodia.tar.gz", server.base_url());
    let dest = seed_partial(dir.path(), ARTIFACT, &url, Some(ETAG))?;

    let log = ProgressLog::default();
    download_to_file(&reqwest::Client::new(), &url, VERSION, SIZE, &dest, &log.sink()).await?;

    assert!(server.requests().is_empty(), "a skipped download must not reach the network");
    assert_eq!(log.seen(), vec![100], "the bar still completes so the UI doesn't stall");
    Ok(())
}

/// A resumed download's `Content-Length` is what remains, not what the file will hold. Measuring
/// against it restarts the bar at zero on a transfer that is most of the way done.
#[tokio::test]
async fn progress_is_measured_against_the_manifest_size() -> TestResult {
    let server = TestServer::start(|req| match req.header("range") {
        Some("bytes=8-") => {
            TestResponse::status(206).header("Content-Range", "bytes 8-9/10").body(&ARTIFACT[8..])
        }
        _ => TestResponse::status(500),
    })?;

    let dir = staging()?;
    let url = format!("{}/melodia.tar.gz", server.base_url());
    let dest = seed_partial(dir.path(), &ARTIFACT[..8], &url, Some(ETAG))?;

    let log = ProgressLog::default();
    download_to_file(&reqwest::Client::new(), &url, VERSION, SIZE, &dest, &log.sink()).await?;

    let seen = log.seen();
    assert_eq!(seen.first(), Some(&80), "the bar opens where the file already is: {seen:?}");
    assert_eq!(seen.last(), Some(&100), "and still reaches the end: {seen:?}");
    Ok(())
}

/// Without a sidecar the bytes on disk belong to nothing identifiable, so they are discarded
/// rather than resumed — and the request that follows carries no `Range` at all.
#[tokio::test]
async fn a_partial_with_no_sidecar_is_discarded_rather_than_resumed() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok(ARTIFACT))?;

    let dir = staging()?;
    let url = format!("{}/melodia.tar.gz", server.base_url());
    let dest = dir.path().join("Melodia.new");
    std::fs::write(&dest, &ARTIFACT[..4])?;

    let log = ProgressLog::default();
    download_to_file(&reqwest::Client::new(), &url, VERSION, SIZE, &dest, &log.sink()).await?;

    let sent = server.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].header("range"), None, "unvouched bytes must not seed a range request");
    assert_eq!(std::fs::read(&dest)?, ARTIFACT);
    Ok(())
}

/// A non-200 is a transport failure rather than a validation one, which is what puts the right
/// toast in front of the user.
#[tokio::test]
async fn a_failed_response_is_reported_as_a_network_error() -> TestResult {
    let server = TestServer::start(|_| TestResponse::status(403))?;

    let dir = staging()?;
    let url = format!("{}/melodia.tar.gz", server.base_url());
    let dest = dir.path().join("Melodia.new");

    let log = ProgressLog::default();
    let outcome =
        download_to_file(&reqwest::Client::new(), &url, VERSION, SIZE, &dest, &log.sink()).await;

    assert!(
        matches!(outcome, Err(melodia_core::error::AppError::Network { .. })),
        "got {outcome:?}"
    );
    Ok(())
}
