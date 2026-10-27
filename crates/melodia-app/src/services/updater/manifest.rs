//! `latest.json`, its `platforms` keyed by target (`linux-x86_64-appimage`, …), each asset
//! inlining its full `.minisig` text as `scripts/build-latest-json.py` writes it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Bumped only when [`LatestManifest`] changes in a way older clients can't read; a field added
/// under `#[serde(default)]` doesn't.
pub const SUPPORTED_MANIFEST_SCHEMA: u32 = 1;

fn default_manifest_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestManifest {
    #[serde(default = "default_manifest_schema_version")]
    pub manifest_schema_version: u32,
    pub version: String,
    /// Hides "Skip this version", for a fix that can't be deferred. Set per release in CI.
    #[serde(default)]
    pub critical: bool,
    #[serde(default)]
    pub pub_date: String,
    #[serde(default)]
    pub notes_short: String,
    pub platforms: HashMap<String, PlatformAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformAsset {
    pub url: String,
    /// Full multi-line `.minisig` contents, parsed by `minisign_verify::Signature::decode`.
    pub signature: String,
    pub size: u64,
}

#[cfg(test)]
#[path = "tests/manifest_tests.rs"]
mod tests;
