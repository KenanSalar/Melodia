//! Top-level "is there a newer version?" query.
//!
//! [`check_for_update`] is the whole surface: manifest fetch + semver gate +
//! platform-asset resolution. Notifying the user is deliberately not here —
//! `tasks::updater_daily` owns the cadence and pushes toasts over its
//! `event_tx`, so this stays a pure query.

use melodia_core::error::{AppError, AppResult};

use super::github::{FetchOutcome, fetch_latest_manifest};
use super::manifest::{self, LatestManifest, PlatformAsset};
use super::version::is_upgrade;
use melodia_platform::services::platform::install_kind::target::current_target_key;

#[derive(Debug, Clone)]
pub enum CheckOutcome {
    UpToDate,
    NotModified,
    /// Manifest returned a version this client can serve an update for.
    /// `asset` is the platform-specific entry from `manifest.platforms`,
    /// pre-resolved via [`current_target_key`].
    Available {
        manifest: LatestManifest,
        asset: PlatformAsset,
        etag: Option<String>,
    },
    /// Manifest was fetched and parsed but didn't list an asset for the
    /// current platform (e.g. ARM Linux against an x86_64-only release).
    /// Treated as "up to date" by callers — there's nothing to install —
    /// but the etag is still cached.
    NoAssetForTarget {
        etag: Option<String>,
    },
    /// Manifest declared a `manifest_schema_version` greater than the
    /// constant the running binary understands ([`manifest::SUPPORTED_MANIFEST_SCHEMA`]).
    /// Treated as "up to date" by callers — the user's current binary
    /// can't safely interpret the manifest — but logged and the etag is
    /// still cached so we don't re-parse it every check.
    UnsupportedSchema {
        schema: u32,
        etag: Option<String>,
    },
}

/// Fetch `latest.json`, semver-gate against the running binary's
/// version, and resolve the platform-specific asset.
///
/// `current_version` is normally `env!("CARGO_PKG_VERSION")` and `base_url`
/// [`RELEASES_BASE`](super::github::RELEASES_BASE); both are parameters for the
/// same reason, which is that a caller can be a test.
///
/// `force_refresh` controls whether the inner [`fetch_latest_manifest`]
/// sends `If-None-Match`. Daily-task checks pass `false` (cache-friendly,
/// the 6h cadence + `ETag` short-circuit keeps GitHub bandwidth flat).
/// User-initiated checks (the "Check for updates" button in the Settings
/// panel) pass `true` so a sticky `UnsupportedSchema` outcome cleared
/// by a re-uploaded manifest takes effect immediately rather than after
/// the next manifest publish bumps the `ETag`.
pub async fn check_for_update(
    http: &reqwest::Client,
    base_url: &str,
    cached_etag: Option<&str>,
    current_version: &str,
    force_refresh: bool,
) -> AppResult<CheckOutcome> {
    let outcome = fetch_latest_manifest(http, base_url, cached_etag, force_refresh).await?;
    let (manifest, etag) = match outcome {
        FetchOutcome::NotModified => return Ok(CheckOutcome::NotModified),
        FetchOutcome::Fresh { manifest, etag } => (manifest, etag),
    };

    classify_manifest(manifest, etag, current_version, current_target_key())
}

/// The ladder itself, split from the fetch so every outcome is reachable without a network or a
/// signature — the host answers `current_target_key` with one value, which left the `None` arm and
/// five of the six package keys unexercised wherever the suite runs.
fn classify_manifest(
    manifest: LatestManifest,
    etag: Option<String>,
    current_version: &str,
    target_key: Option<&str>,
) -> AppResult<CheckOutcome> {
    // Forward-compat gate: a future schema revision (renamed platform
    // keys, restructured PlatformAsset, etc.) bumps the manifest's
    // `manifest_schema_version` past our compiled-in `SUPPORTED_*` value.
    // We refuse to act on a manifest we don't fully understand rather
    // than guessing — the user's binary can't safely interpret the new
    // shape, and they need to upgrade out-of-band (download from the
    // GitHub release page) before the in-app updater becomes useful
    // again. Cache the etag so we don't re-pay the JSON parse on every
    // 6-hourly check.
    if manifest.manifest_schema_version > manifest::SUPPORTED_MANIFEST_SCHEMA {
        log::warn!(
            "updater: manifest_schema_version={} exceeds supported={} — \
             refusing to install until this binary is upgraded out-of-band",
            manifest.manifest_schema_version,
            manifest::SUPPORTED_MANIFEST_SCHEMA
        );
        return Ok(CheckOutcome::UnsupportedSchema {
            schema: manifest.manifest_schema_version,
            etag,
        });
    }

    if !is_upgrade(current_version, &manifest.version)? {
        return Ok(CheckOutcome::UpToDate);
    }

    let Some(key) = target_key else {
        return Ok(CheckOutcome::NoAssetForTarget { etag });
    };
    let Some(asset) = manifest.platforms.get(key).cloned() else {
        return Err(AppError::Validation(format!(
            "manifest reports version {} but has no platform asset for '{key}'",
            manifest.version
        )));
    };

    Ok(CheckOutcome::Available {
        manifest,
        asset,
        etag,
    })
}

#[cfg(test)]
#[path = "tests/check_tests.rs"]
mod tests;
