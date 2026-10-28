//! The Now Playing view's [`crate::ui::artwork_cache`] tier.

use std::num::NonZeroUsize;
use std::path::Path;

use crate::ui::artwork_cache::{ArtworkCache, BlurSpec, CachedArtwork};
use crate::ui::util::{BLUR_SIGMA, BLUR_TARGET};

/// Up Next surfaces about twenty tracks the user skips through, and a smaller cap thrashes on
/// exactly that.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(8) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// Square, at the full wash: nothing is painted over it, and the view is as tall as it is wide.
const BLUR: BlurSpec = BlurSpec { height: BLUR_TARGET, sigma: BLUR_SIGMA };

/// This tier's spec, or `None` under the aurora.
pub fn blur_spec(ui: &melodia_ui::AppWindow) -> Option<BlurSpec> {
    crate::ui::backdrop::blur_spec(ui, BLUR)
}

pub struct NowPlayingArtwork(ArtworkCache);

impl NowPlayingArtwork {
    /// Takes the spec so tests can build a tier without a window; production passes [`blur_spec`].
    pub fn new(blur: Option<BlurSpec>) -> Self {
        Self(ArtworkCache::new(ARTWORK_CACHE_CAP, blur))
    }

    /// See [`ArtworkCache::get_or_decode`].
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        self.0.get_or_decode(path)
    }

    /// Called when the Now Playing view closes.
    pub fn clear(&self) {
        self.0.clear();
    }
}
