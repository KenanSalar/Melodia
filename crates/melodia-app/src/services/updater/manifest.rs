//! Serde types for `latest.json`.
//!
//! The `platforms` map is keyed by target (`linux-x86_64-appimage`, …), and each
//! asset's `signature` carries the full multi-line `.minisig` text, inlined by
//! `scripts/build-latest-json.py` at release time — serde preserves the embedded
//! `\n`s through round-trip.
//!
//! `manifest_schema_version` is the forward-compat gate: a future break — renamed
//! platform keys, a restructured asset, a new required field — bumps it, and clients
//! running an old binary refuse to parse rather than panicking on a deserialise
//! mismatch. Defaults to 1 for any in-flight pre-versioned manifest.
//!
//! `critical` suppresses the "Skip this version" button, for a security fix the user
//! shouldn't be able to defer indefinitely. The notification toast is still
//! dismissable, but it re-appears at the next daily check.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Highest `manifest_schema_version` this binary understands. Bump only when
/// [`LatestManifest`]'s shape changes in a way that breaks older clients — adding an
/// optional `#[serde(default)]` field does not.
pub const SUPPORTED_MANIFEST_SCHEMA: u32 = 1;

fn default_manifest_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestManifest {
    #[serde(default = "default_manifest_schema_version")]
    pub manifest_schema_version: u32,
    pub version: String,
    /// `true` ⇒ the UI hides "Skip this version" so the release can't be deferred
    /// permanently. Set by the publisher per-release in CI, not derived from the notes.
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
