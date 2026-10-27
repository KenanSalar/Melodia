//! The detail band's [`crate::ui::artwork_cache`] tier, and the [`DetailPair`] three detail views
//! carry to the UI thread.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::ui::artwork_cache::{ArtworkCache, ArtworkPair, BlurSpec, CachedArtwork};
use crate::ui::backdrop::BackdropSample;
use melodia_app::state::AppState;
use melodia_core::error::describe;

/// Landscape, and lighter than Now Playing's wash: the band is panel-wide by `Theme.hero-artwork`
/// tall, under a floor and a scrim. Shared with the curated heroes, which draw the same band from a
/// composed source with no path to key a cache on.
pub(crate) const BLUR: BlurSpec = BlurSpec { height: 85, sigma: 13.3 };

/// The open entity plus a handful opened recently, for the back-and-forth.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(12) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// This tier's spec, or `None` under the aurora, paired here once for the three detail views.
pub fn blur_spec(ui: &melodia_ui::AppWindow) -> Option<BlurSpec> {
    crate::ui::backdrop::blur_spec(ui, BLUR)
}

pub struct DetailArtwork(ArtworkCache);

impl DetailArtwork {
    /// Takes the spec so tests can build a tier without a window; production passes [`blur_spec`].
    pub fn new(blur: Option<BlurSpec>) -> Self {
        Self(ArtworkCache::new(ARTWORK_CACHE_CAP, blur))
    }

    /// See [`ArtworkCache::get_or_decode`].
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        self.0.get_or_decode(path)
    }

    /// Called when the user leaves the section.
    pub fn clear(&self) {
        self.0.clear();
    }
}

/// A decoded cover and blur as `Send` buffers, for the hop to the UI thread. Empty for missing
/// artwork, the publisher then falling back to the accent and the floor's own luma.
#[derive(Clone, Default)]
pub(crate) struct DetailPair {
    pub(crate) cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
    pub(crate) blur: Option<SharedPixelBuffer<Rgb8Pixel>>,
    pub(crate) sample: BackdropSample,
}

impl From<ArtworkPair> for DetailPair {
    fn from(pair: ArtworkPair) -> Self {
        Self { cover: Some(pair.cover), blur: pair.blur, sample: pair.sample }
    }
}

/// Decodes an entity's artwork into a [`DetailPair`] on the blocking pool. Empty for a missing or
/// undecodable path; the caller clears `has-blur` so the floor shows through.
pub(crate) async fn decode_detail_pair(
    state: &AppState,
    artwork: Arc<DetailArtwork>,
    path: Option<String>,
) -> DetailPair {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return DetailPair::default();
    };
    match state.runtime.spawn_blocking(move || artwork.get_or_decode(Path::new(&path))).await {
        Ok(Some(pair)) => pair.into(),
        Ok(None) => DetailPair::default(),
        Err(e) => {
            log::warn!("detail artwork decode: {}", describe(&e));
            DetailPair::default()
        }
    }
}
