//! In-memory cache of the most-recently-observed `Available` asset.
//!
//! [`Updater.install`](crate::ui::callbacks::wire_updater) re-fetches
//! `latest.json` to pick up the asset blob (URL + signature + size)
//! before downloading. The re-fetch is cheap on the happy path (the
//! cached `ETag` usually short-circuits to 304), but fails outright when
//! the user is on a flaky network — and a failure between "user saw
//! the Available toast" and "user clicked Install" produced a
//! confusing `"re-fetch before install failed: …"` toast.
//!
//! This cache holds the last `PlatformAsset` from a successful
//! `Available` outcome (manual check, daily task, or the install
//! flow's own re-fetch). `spawn_install` consults the cache as a
//! fallback when the live re-fetch fails — the cached asset stays
//! valid as long as the release hasn't been retracted, and the
//! signature check downstream catches any URL drift.
//!
//! Lifetime: process-scoped. Cleared on `Installed` to avoid
//! re-using a stale asset across an in-process re-check (we shouldn't
//! be installing the same artifact twice). Never persisted to disk —
//! `last_known_release` in settings tracks the version label for the
//! UI, not the download target.

use std::sync::OnceLock;

use parking_lot::Mutex;

use super::manifest::PlatformAsset;

/// The cached entry pairs the asset with the manifest version it was
/// observed under. The version is forwarded to
/// [`super::install::download_and_install`] for the trusted-comment
/// cross-check in `verify_stream` — pinning the asset to its version
/// at observation time means the fallback can't trip the cross-check
/// just because we lost track of what version that asset belonged to.
#[derive(Debug, Clone)]
pub struct CachedAsset {
    pub version: String,
    pub asset: PlatformAsset,
}

static CACHE: OnceLock<Mutex<Option<CachedAsset>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<CachedAsset>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Replace the cached asset with `(version, asset)`. Called from every
/// place that observes a `CheckOutcome::Available` (manual check, daily
/// task, install re-fetch's own Available branch).
pub fn store(version: String, asset: PlatformAsset) {
    *slot().lock() = Some(CachedAsset { version, asset });
}

/// Return a clone of the cached entry, or `None` if nothing has been
/// observed yet this process. The clone is cheap relative to the
/// network round-trip it spares.
pub fn snapshot() -> Option<CachedAsset> {
    slot().lock().clone()
}

/// Drop the cached asset. Called after a successful install so the
/// cache can't fool a subsequent click into re-downloading the same
/// artifact (the live re-fetch would catch this anyway, but explicit
/// clear keeps the state machine clean).
pub fn clear() {
    *slot().lock() = None;
}
