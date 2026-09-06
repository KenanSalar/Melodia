//! Fetches `latest.json` from
//! `https://github.com/KenanSalar/Melodia/releases/latest/download/latest.json`,
//! plus its sibling `latest.json.minisig` for manifest verification.
//!
//! Sends `If-None-Match: <last_manifest_etag>` when the caller has a
//! cached `ETag`, so the daily-check loop can short-circuit on `304 Not
//! Modified` without re-parsing the JSON or re-running state transitions
//! through `Checking` → `UpToDate`.
//!
//! Verification ordering: minisign signature over the manifest body
//! is checked **before** `serde_json::from_str`. A missing `.minisig`,
//! an invalid signature, or a trusted-comment lacking `manifest=true`
//! all return [`AppError::Validation`] from this layer — the caller
//! treats them the same way as a malformed JSON body. Failing closed is
//! the only way the signature is a real security boundary; a fail-open
//! posture would let a network attacker strip the `.minisig` to bypass
//! verification.

use reqwest::StatusCode;
use reqwest::header::{ETAG, HeaderValue, IF_NONE_MATCH};

use melodia_core::error::{AppError, AppResult};

use super::manifest::LatestManifest;
use super::minisign;

/// Where the manifest and its signature are published, threaded through the fetch rather than
/// read from a const so a test can point it at a local server. Parameterised the way
/// `current_version` already is, and deliberately not an environment override: the fetch is the
/// updater's trust boundary, and a second configuration surface on it is one more thing that has
/// to be argued about.
pub const RELEASES_BASE: &str = "https://github.com/KenanSalar/Melodia/releases/latest/download";

/// Ceiling on the manifest body. Room for many times the release
/// matrix it describes, and still a bound: the signature is checked
/// *after* the download, so an unbounded read here would allocate
/// whatever the CDN in front of the release served.
const MANIFEST_MAX_BYTES: u64 = 256 * 1024;

/// A minisign blob is two comment lines and a base64 signature.
const SIGNATURE_MAX_BYTES: u64 = 8 * 1024;

#[derive(Debug)]
pub enum FetchOutcome {
    NotModified,
    Fresh {
        manifest: LatestManifest,
        etag: Option<String>,
    },
}

pub async fn fetch_latest_manifest(
    http: &reqwest::Client,
    base_url: &str,
    cached_etag: Option<&str>,
    force_refresh: bool,
) -> AppResult<FetchOutcome> {
    let manifest_url = format!("{base_url}/latest.json");
    let signature_url = format!("{manifest_url}.minisig");

    let mut req = http.get(&manifest_url);
    // `force_refresh` skips the `If-None-Match` header so the server
    // returns the full manifest body even when our cached ETag is
    // current. Used by the UI "Check for updates" button to clear a
    // sticky `UnsupportedSchema` state — if a release accidentally
    // bumped `manifest_schema_version` ahead of the client rollout,
    // the etag would keep returning 304 forever and the user would
    // see "schema too new" on every check until the maintainer re-
    // uploaded `latest.json`. Manual checks bypass the cache; the
    // daily-task path keeps sending `If-None-Match` to spare GitHub
    // the bandwidth on the 99% no-op case.
    if !force_refresh
        && let Some(tag) = cached_etag
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
        )));
    }

    let etag = resp.headers().get(ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let body =
        melodia_net::services::net::read_capped(resp, "latest.json", MANIFEST_MAX_BYTES).await?;

    // Fetch the sibling .minisig. A missing or non-200 response is
    // treated as a verification failure — we'd rather refuse to install
    // than let a network attacker strip the signature and serve a
    // doctored manifest. The same release that started signing the
    // manifest also ships this verification, so there's no transition
    // gap where a signed-manifest server would serve a pre-signing
    // client (or vice versa) — see the rollout note in the plan.
    let sig_resp = http
        .get(&signature_url)
        .send()
        .await
        .map_err(|e| AppError::network("fetch latest.json.minisig failed", e))?;
    if !sig_resp.status().is_success() {
        return Err(AppError::Validation(format!(
            "manifest signature missing or unreachable: HTTP {} on latest.json.minisig",
            sig_resp.status()
        )));
    }
    let sig_bytes = melodia_net::services::net::read_capped(
        sig_resp,
        "latest.json.minisig",
        SIGNATURE_MAX_BYTES,
    )
    .await?;
    let sig_text = String::from_utf8_lossy(&sig_bytes);

    let pubkey = minisign::embedded_pubkey()
        .map_err(|e| AppError::Validation(format!("embedded updater pubkey is invalid: {e}")))?;
    // Cross-check `manifest=true` to keep an artifact .minisig from
    // being swapped into this slot (both are valid minisign blobs signed
    // with the same key; only the trusted-comment domain separates
    // them). `expected_version` stays None — the manifest *is* the source
    // of the version string, so there's nothing meaningful to assert
    // here until after we parse it.
    minisign::verify_manifest_bytes(&body, &sig_text, &pubkey, None, Some("true")).map_err(
        |e| AppError::Validation(format!("manifest signature verification failed: {e}")),
    )?;

    let manifest: LatestManifest = serde_json::from_slice(&body).map_err(|e| {
        AppError::Validation(format!(
            "parse latest.json failed: {e}; body: {}",
            String::from_utf8_lossy(&body)
        ))
    })?;

    Ok(FetchOutcome::Fresh { manifest, etag })
}

#[cfg(test)]
#[path = "tests/github_tests.rs"]
mod tests;
