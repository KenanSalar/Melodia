//! Album-art decode + cache for the full-screen Now Playing view.
//!
//! The view needs the active cover in two forms — a sharp tile and a
//! heavily-blurred backdrop — so it takes a [`crate::ui::artwork_cache`] tier
//! configured for its own working set. Everything about how that cache
//! behaves is documented there; this file is the two numbers that make it the
//! Now Playing one.

use std::num::NonZeroUsize;
use std::path::Path;

use crate::ui::artwork_cache::{ArtworkCache, BlurSpec, CachedArtwork};
use crate::ui::util::{BLUR_SIGMA, BLUR_TARGET};

/// LRU capacity. "Up Next" surfaces ~20 tracks and the user skips through
/// neighbours, so too small a cap thrashes on exactly the interaction the
/// feature exists for; this covers the realistic working set while keeping
/// the resident `(cover, blur)` pairs bounded.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(8) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// Square, at the full wash: this backdrop has nothing painted over it, and
/// the view it fills is as tall as it is wide.
const BLUR: BlurSpec = BlurSpec {
    height: BLUR_TARGET,
    sigma: BLUR_SIGMA,
};

pub struct NowPlayingArtwork(ArtworkCache);

impl Default for NowPlayingArtwork {
    fn default() -> Self {
        Self(ArtworkCache::new(ARTWORK_CACHE_CAP, BLUR))
    }
}

impl NowPlayingArtwork {
    pub fn new() -> Self {
        Self::default()
    }

    /// See [`ArtworkCache::get_or_decode`].
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        self.0.get_or_decode(path)
    }

    /// Drop every cached pair — called when the Now Playing view closes.
    pub fn clear(&self) {
        self.0.clear();
    }
}
