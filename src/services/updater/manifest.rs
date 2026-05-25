//! Serde types for `latest.json`.
//!
//! Shape (per platform asset):
//! ```json
//! {
//!   "manifest_schema_version": 1,
//!   "version": "0.2.0",
//!   "critical": false,
//!   "pub_date": "2026-06-01T12:00:00Z",
//!   "notes_short": "Bug fixes and stability improvements.",
//!   "platforms": {
//!     "linux-x86_64-appimage": {
//!       "url": "https://…/melodia-0.2.0-x86_64.AppImage",
//!       "signature": "untrusted comment: …\n<sig>\ntrusted comment: …\n<global>",
//!       "size": 52428800
//!     }
//!   }
//! }
//! ```
//!
//! The `signature` field carries the full multi-line `.minisig` text
//! (inlined by `scripts/build-latest-json.py` at release time). serde
//! preserves the embedded `\n`s through round-trip — verified by
//! `manifest_tests::round_trip_preserves_multiline_signature`.
//!
//! `manifest_schema_version` is the forward-compat gate: a future schema
//! break (renamed platform keys, restructured platform asset, new required
//! field) bumps this value and clients running an old binary refuse to
//! parse rather than panicking on a deserialise mismatch. Defaults to 1
//! for back-compat with any in-flight pre-versioned manifest (the field
//! was added in the same release that started emitting it; today's
//! production manifest carries it explicitly).
//!
//! `critical` suppresses the "Skip this version" button in the Settings →
//! Updates panel. Set to `true` for security fixes the user shouldn't be
//! able to defer indefinitely. The notification toast is still
//! dismissable, but it re-appears at the next daily check.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Highest `manifest_schema_version` the current binary understands.
/// Bump only when [`LatestManifest`]'s shape changes in a way that
/// would break older clients. Adding optional `#[serde(default)]` fields
/// is back-compat and does NOT require a bump.
pub const SUPPORTED_MANIFEST_SCHEMA: u32 = 1;

fn default_manifest_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestManifest {
    #[serde(default = "default_manifest_schema_version")]
    pub manifest_schema_version: u32,
    pub version: String,
    /// `true` ⇒ UI hides the "Skip this version" action so the user
    /// can't permanently defer the release. Independent of severity in
    /// release notes — set by the publisher per-release in CI.
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
    /// Full multi-line `.minisig` contents. Parsed via
    /// `minisign_verify::Signature::decode`.
    pub signature: String,
    pub size: u64,
}

#[cfg(test)]
#[path = "tests/manifest_tests.rs"]
mod tests;
