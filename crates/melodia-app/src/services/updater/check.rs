//! Whether a newer version is published, as a pure query: recording the answer and notifying are
//! [`run_check`](super::run_check)'s.

use melodia_core::error::AppError;

use super::event::{UpdateFailure, UpdateResult};
use super::github::{FetchOutcome, fetch_latest_manifest};
use super::manifest::{self, LatestManifest, PlatformAsset};
use super::version::is_upgrade;
use melodia_platform::services::platform::install_kind::target::current_target_key;

/// A manifest read in full and judged against the running binary.
#[derive(Debug, Clone)]
pub struct Checked {
    pub manifest: LatestManifest,
    pub etag: Option<String>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone)]
pub enum Verdict {
    UpToDate,
    /// Newer, with no asset for this platform.
    NoAssetForTarget,
    /// The manifest's schema is past [`manifest::SUPPORTED_MANIFEST_SCHEMA`].
    UnsupportedSchema,
    /// Newer and installable, with this target's asset.
    Available(PlatformAsset),
}

/// Judges the published manifest against `current_version`, normally
/// [`installed_version`](super::install::installed_version); it and `base_url` are parameters for
/// tests.
pub async fn check_for_update(
    http: &reqwest::Client,
    base_url: &str,
    current_version: &str,
) -> UpdateResult<Checked> {
    match fetch_latest_manifest(http, base_url, None).await? {
        FetchOutcome::Fresh { manifest, etag } => judge(manifest, etag, current_version),
        FetchOutcome::NotModified => {
            Err(AppError::network_msg("latest.json answered 304 to a request that sent no ETag")
                .into())
        }
    }
}

/// [`check_for_update`], revalidating against `etag`. `None` when the server answers `304`: the
/// manifest the tag names is still the published one.
pub async fn recheck_for_update(
    http: &reqwest::Client,
    base_url: &str,
    etag: &str,
    current_version: &str,
) -> UpdateResult<Option<Checked>> {
    match fetch_latest_manifest(http, base_url, Some(etag)).await? {
        FetchOutcome::Fresh { manifest, etag } => judge(manifest, etag, current_version).map(Some),
        FetchOutcome::NotModified => Ok(None),
    }
}

fn judge(
    manifest: LatestManifest,
    etag: Option<String>,
    current_version: &str,
) -> UpdateResult<Checked> {
    let verdict = classify_manifest(&manifest, current_version, current_target_key())?;
    Ok(Checked { manifest, etag, verdict })
}

/// Split from the fetch so every verdict is reachable from a test, the host answering
/// `current_target_key` with one value.
fn classify_manifest(
    manifest: &LatestManifest,
    current_version: &str,
    target_key: Option<&str>,
) -> UpdateResult<Verdict> {
    // A newer schema may have reshaped anything, so none of it is acted on.
    if manifest.manifest_schema_version > manifest::SUPPORTED_MANIFEST_SCHEMA {
        log::warn!(
            "updater: manifest_schema_version={} exceeds supported={} — \
             refusing to install until this binary is upgraded out-of-band",
            manifest.manifest_schema_version,
            manifest::SUPPORTED_MANIFEST_SCHEMA
        );
        return Ok(Verdict::UnsupportedSchema);
    }

    // Only the manifest's version can fail to parse: the running one is this build's, or one it
    // installed from an earlier manifest.
    if !is_upgrade(current_version, &manifest.version).map_err(UpdateFailure::parse)? {
        return Ok(Verdict::UpToDate);
    }

    let Some(key) = target_key else {
        return Ok(Verdict::NoAssetForTarget);
    };
    let Some(asset) = manifest.platforms.get(key).cloned() else {
        return Err(UpdateFailure::parse(AppError::updater_msg(format!(
            "manifest reports version {} but has no platform asset for '{key}'",
            manifest.version
        ))));
    };

    Ok(Verdict::Available(asset))
}

#[cfg(test)]
#[path = "tests/check_tests.rs"]
mod tests;
