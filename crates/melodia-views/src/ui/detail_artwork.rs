//! Album-art decode + cache for the band's hero half.
//!
//! A detail hero needs the active cover in two forms — a sharp artwork tile
//! and a heavily-blurred, full-bleed backdrop — so it takes a
//! [`crate::ui::artwork_cache`] tier configured for its own working set.
//! Everything about how that cache behaves is documented there; this file is
//! the three numbers that make it the detail one, plus the [`DetailPair`] the
//! three artwork-bearing detail views carry from the worker to the UI thread.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::ui::artwork_cache::{ArtworkCache, ArtworkPair, BlurSpec, CachedArtwork};
use crate::ui::backdrop::BackdropSample;
use melodia_app::state::AppState;

/// Landscape, and lighter than the Now Playing tier's wash: the hero region
/// paints full content-panel width by a band `Theme.hero-artwork` tall, and
/// the gradient floor plus the solved scrim sit on top of this blur, so it
/// needs less of its own.
///
/// Public because the curated heroes want it without wanting this tier's LRU — their
/// source is composed per refresh, so there is no path to key it on, but they draw the
/// same band shape and owe it the same blur.
pub(crate) const BLUR: BlurSpec = BlurSpec {
    height: 85,
    sigma: 13.3,
};

/// LRU capacity. The working set for a detail view is the currently-open
/// entity plus a handful of recently-opened ones (the back-and-forth
/// pattern). At one `(cover, blur)` pair per entry ≈ `432 KiB + 32 KiB`,
/// 12 entries caps at ≈ 5.5 MiB — comfortably under the RSS ceiling.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(12) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// This tier's spec, or `None` under the aurora setting. The three detail views each build
/// their own tier, so the pairing of [`BLUR`] with the live setting lives here rather than
/// three times at their `install`s.
pub fn blur_spec(ui: &melodia_ui::AppWindow) -> Option<BlurSpec> {
    crate::ui::backdrop::blur_spec(ui, BLUR)
}

pub struct DetailArtwork(ArtworkCache);

impl DetailArtwork {
    /// Takes the spec rather than reading the setting itself, so the tests can build a tier
    /// without a window. [`blur_spec`] is what the three production callers pass.
    pub fn new(blur: Option<BlurSpec>) -> Self {
        Self(ArtworkCache::new(ARTWORK_CACHE_CAP, blur))
    }

    /// See [`ArtworkCache::get_or_decode`].
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        self.0.get_or_decode(path)
    }

    /// Drop every cached pair — called when the user leaves the section.
    pub fn clear(&self) {
        self.0.clear();
    }
}

/// A decoded cover + blur pair as raw RGB8 buffers, both `Send` so they
/// cross the `upgrade_in_event_loop` boundary. The detail views carry this
/// from the worker to the UI thread, which wraps each half in a
/// `slint::Image` via `Image::from_rgb8` (`Image` itself is `!Send`, hence
/// the raw-buffer form). `None` on either buffer means missing /
/// failed-decode artwork, and `sample` is then empty too — the publisher
/// falls back to the theme accent and the gradient floor's own luma.
#[derive(Clone, Default)]
pub(crate) struct DetailPair {
    pub(crate) cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
    pub(crate) blur: Option<SharedPixelBuffer<Rgb8Pixel>>,
    pub(crate) sample: BackdropSample,
}

impl From<ArtworkPair> for DetailPair {
    fn from(pair: ArtworkPair) -> Self {
        Self {
            cover: Some(pair.cover),
            blur: pair.blur,
            sample: pair.sample,
        }
    }
}

/// Decode an entity's artwork into a [`DetailPair`] for a detail-view
/// header — the sharp header tile **and** the heavily-blurred hero
/// backdrop, both derived from one source decode. Off-loaded to the
/// `spawn_blocking` pool (image decode, box blur and the backdrop
/// measurement are all CPU-bound). Returns an empty pair for a missing /
/// empty / failed-decode path; the caller clears `has-blur` so the gradient
/// floor shows through. Shared by every detail view (Album / Artist /
/// Playlist).
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
            log::warn!("detail artwork decode: {e}");
            DetailPair::default()
        }
    }
}
