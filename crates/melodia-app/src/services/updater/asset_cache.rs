//! The last `Available` asset this process saw, which an Install click falls back to when its own
//! re-fetch fails. It stays valid until the release is retracted, the signature check catching any
//! drift, and is never persisted.

use std::sync::OnceLock;

use parking_lot::Mutex;

use super::manifest::PlatformAsset;

/// The asset with the version it was seen under, the pair `verify_stream`'s version check needs.
#[derive(Debug, Clone)]
pub struct CachedAsset {
    pub version: String,
    pub asset: PlatformAsset,
}

static CACHE: OnceLock<Mutex<Option<CachedAsset>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<CachedAsset>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

pub fn store(version: String, asset: PlatformAsset) {
    *slot().lock() = Some(CachedAsset { version, asset });
}

pub fn snapshot() -> Option<CachedAsset> {
    slot().lock().clone()
}

/// After an install, so a later click can't download the same artifact again.
pub fn clear() {
    *slot().lock() = None;
}
