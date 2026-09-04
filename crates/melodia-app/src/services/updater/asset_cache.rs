//! In-memory cache of the most-recently-observed `Available` asset.
//!
//! `Updater.install` (`ui::callbacks::wire_updater`) re-fetches `latest.json`
//! for the asset blob (URL + signature + size) before downloading. That is cheap on
//! the happy path — the cached `ETag` usually 304s — but fails outright on a flaky
//! network, and a failure between the Available toast and the Install click surfaced
//! as a confusing re-fetch error.
//!
//! So `spawn_install` falls back to the last `PlatformAsset` from a successful
//! `Available` outcome: it stays valid as long as the release hasn't been retracted,
//! and the signature check downstream catches any URL drift. Process-scoped, cleared
//! on `Installed` so a stale asset can't survive into an in-process re-check, and
//! never persisted — `last_known_release` tracks the version label for the UI, not
//! the download target.

use std::sync::OnceLock;

use parking_lot::Mutex;

use super::manifest::PlatformAsset;

/// Pairs the asset with the manifest version it was observed under, which
/// [`super::install::download_and_install`] forwards to `verify_stream`'s
/// trusted-comment cross-check: pinning the two together at observation time is what
/// stops the fallback tripping that check over a version it merely lost track of.
#[derive(Debug, Clone)]
pub struct CachedAsset {
    pub version: String,
    pub asset: PlatformAsset,
}

static CACHE: OnceLock<Mutex<Option<CachedAsset>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<CachedAsset>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Replace the cached asset. Called from everything that observes a
/// `CheckOutcome::Available` — manual check, daily task, install re-fetch.
pub fn store(version: String, asset: PlatformAsset) {
    *slot().lock() = Some(CachedAsset { version, asset });
}

/// A clone of the cached entry, or `None` if nothing has been observed yet this
/// process. Cheap relative to the network round-trip it spares.
pub fn snapshot() -> Option<CachedAsset> {
    slot().lock().clone()
}

/// Drop the cached asset after a successful install, so a later click can't be
/// fooled into re-downloading the same artifact.
pub fn clear() {
    *slot().lock() = None;
}
