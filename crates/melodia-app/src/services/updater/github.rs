//! Fetches `latest.json` and its `.minisig`, sending `If-None-Match` when the caller has a tag so
//! a `304` skips both the fetch and the verify.
//!
//! The signature is checked **before** the body is parsed, and fails closed: a missing `.minisig`
//! is refused like an invalid one, or a network attacker could strip it.

use reqwest::StatusCode;
use reqwest::header::{ETAG, HeaderValue, IF_NONE_MATCH};

use melodia_core::error::AppError;

use super::event::{UpdateFailure, UpdateResult};
use super::manifest::LatestManifest;
use super::minisign;

/// Passed to the fetch rather than read inside it, so a test can point it at a local server.
/// Deliberately no environment override: the fetch is the updater's trust boundary.
pub const RELEASES_BASE: &str = "https://github.com/KenanSalar/Melodia/releases/latest/download";

/// Many times the release matrix, and still a bound: the signature is checked only after the read.
const MANIFEST_MAX_BYTES: u64 = 256 * 1024;

/// A minisign blob is two comment lines and a base64 signature.
const SIGNATURE_MAX_BYTES: u64 = 8 * 1024;

#[derive(Debug)]
pub enum FetchOutcome {
    NotModified,
    Fresh { manifest: LatestManifest, etag: Option<String> },
}

pub async fn fetch_latest_manifest(
    http: &reqwest::Client,
    base_url: &str,
    cached_etag: Option<&str>,
) -> UpdateResult<FetchOutcome> {
    let manifest_url = format!("{base_url}/latest.json");
    let signature_url = format!("{manifest_url}.minisig");

    let mut req = http.get(&manifest_url);
    if let Some(tag) = cached_etag
        && !tag.is_empty()
        && let Ok(value) = HeaderValue::from_str(tag)
    {
        req = req.header(IF_NONE_MATCH, value);
    }

    let resp = req.send().await.map_err(|e| AppError::network("fetch latest.json failed", e))?;

    if resp.status() == StatusCode::NOT_MODIFIED {
        return Ok(FetchOutcome::NotModified);
    }
    if !resp.status().is_success() {
        return Err(AppError::network_msg(format!(
            "fetch latest.json returned HTTP {}",
            resp.status()
        ))
        .into());
    }

    let etag = resp.headers().get(ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let body =
        melodia_net::services::net::read_capped(resp, "latest.json", MANIFEST_MAX_BYTES).await?;

    let sig_resp = http
        .get(&signature_url)
        .send()
        .await
        .map_err(|e| AppError::network("fetch latest.json.minisig failed", e))?;
    if !sig_resp.status().is_success() {
        return Err(UpdateFailure::signature(AppError::updater_msg(format!(
            "manifest signature missing or unreachable: HTTP {} on latest.json.minisig",
            sig_resp.status()
        ))));
    }
    let sig_bytes = melodia_net::services::net::read_capped(
        sig_resp,
        "latest.json.minisig",
        SIGNATURE_MAX_BYTES,
    )
    .await?;
    let sig_text = String::from_utf8_lossy(&sig_bytes);

    let pubkey = minisign::embedded_pubkey()
        .map_err(|e| AppError::updater("embedded updater pubkey is invalid", e))?;
    minisign::verify_manifest_bytes(&body, &sig_text, &pubkey, "true").map_err(|e| {
        UpdateFailure::signature(AppError::updater("manifest signature verification failed", e))
    })?;

    // The body stays out of the error: having verified, it is a manifest we published, and the
    // release it came from still carries it.
    let manifest: LatestManifest = serde_json::from_slice(&body)
        .map_err(|e| UpdateFailure::parse(AppError::updater("parse latest.json failed", e)))?;

    Ok(FetchOutcome::Fresh { manifest, etag })
}

#[cfg(test)]
#[path = "tests/github_tests.rs"]
mod tests;
