//! Top-level "is there a newer version?" query.
//!
//! [`check_for_update`] and [`recheck_for_update`] are the whole surface: manifest fetch + semver
//! gate + platform-asset resolution. Recording the answer and notifying the user are
//! [`run_check`](super::run_check)'s, so this stays a pure query.

use melodia_core::error::{AppError, AppResult};

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
    /// A newer release with no asset for the current platform (e.g. ARM Linux against an
    /// x86_64-only release): nothing to install.
    NoAssetForTarget,
    /// The manifest declares a `manifest_schema_version` past
    /// [`manifest::SUPPORTED_MANIFEST_SCHEMA`], so this binary can't safely interpret it.
    UnsupportedSchema,
    /// A newer release this client can install, carrying the running target's entry from
    /// `manifest.platforms` as resolved by [`current_target_key`].
    Available(PlatformAsset),
}

/// Fetch `latest.json` in full, semver-gate it against `current_version`, and resolve the
/// platform-specific asset.
///
/// `current_version` is normally [`installed_version`](super::install::installed_version) and
/// `base_url` [`RELEASES_BASE`](super::github::RELEASES_BASE); both are parameters for the
/// same reason, which is that a caller can be a test.
pub async fn check_for_update(
    http: &reqwest::Client,
    base_url: &str,
    current_version: &str,
) -> AppResult<Checked> {
    match fetch_latest_manifest(http, base_url, None).await? {
        FetchOutcome::Fresh { manifest, etag } => judge(manifest, etag, current_version),
        FetchOutcome::NotModified => {
            Err(AppError::network_msg("latest.json answered 304 to a request that sent no ETag"))
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
) -> AppResult<Option<Checked>> {
    match fetch_latest_manifest(http, base_url, Some(etag)).await? {
        FetchOutcome::Fresh { manifest, etag } => judge(manifest, etag, current_version).map(Some),
        FetchOutcome::NotModified => Ok(None),
    }
}

fn judge(
    manifest: LatestManifest,
    etag: Option<String>,
    current_version: &str,
) -> AppResult<Checked> {
    let verdict = classify_manifest(&manifest, current_version, current_target_key())?;
    Ok(Checked { manifest, etag, verdict })
}

/// The ladder itself, split from the fetch so every outcome is reachable without a network or a
/// signature — the host answers `current_target_key` with one value, which left the `None` arm and
/// five of the six package keys unexercised wherever the suite runs.
fn classify_manifest(
    manifest: &LatestManifest,
    current_version: &str,
    target_key: Option<&str>,
) -> AppResult<Verdict> {
    // Forward-compat gate: a future schema revision (renamed platform
    // keys, restructured PlatformAsset, etc.) bumps the manifest's
    // `manifest_schema_version` past our compiled-in `SUPPORTED_*` value.
    // We refuse to act on a manifest we don't fully understand rather
    // than guessing — the user's binary can't safely interpret the new
    // shape, and they need to upgrade out-of-band (download from the
    // GitHub release page) before the in-app updater becomes useful
    // again.
    if manifest.manifest_schema_version > manifest::SUPPORTED_MANIFEST_SCHEMA {
        log::warn!(
            "updater: manifest_schema_version={} exceeds supported={} — \
             refusing to install until this binary is upgraded out-of-band",
            manifest.manifest_schema_version,
            manifest::SUPPORTED_MANIFEST_SCHEMA
        );
        return Ok(Verdict::UnsupportedSchema);
    }

    if !is_upgrade(current_version, &manifest.version)? {
        return Ok(Verdict::UpToDate);
    }

    let Some(key) = target_key else {
        return Ok(Verdict::NoAssetForTarget);
    };
    let Some(asset) = manifest.platforms.get(key).cloned() else {
        return Err(AppError::Validation(format!(
            "manifest reports version {} but has no platform asset for '{key}'",
            manifest.version
        )));
    };

    Ok(Verdict::Available(asset))
}

#[cfg(test)]
#[path = "tests/check_tests.rs"]
mod tests;
