//! What the two curated heroes share: composing up to four covers into one banner, past which each
//! is an ordinary single-artwork hero on the detail path, and the guard deciding when to recompose.

use std::path::PathBuf;

use parking_lot::Mutex;

use crate::ui::artwork_cache::{BlurSpec, pair_from_image};
use crate::ui::detail_artwork::DetailPair;
use crate::ui::util::COVER_SIZE;
use melodia_app::state::AppState;
use melodia_artwork::media::image::artwork::compose_cover;
use melodia_core::error::describe;

/// [`compose_hero_pair`] on the blocking pool. `None` only when the task failed, and the caller
/// then returns rather than claim the set with an empty pair, so the next tick tries again.
pub(crate) async fn compose_off_thread(
    state: &AppState,
    paths: Vec<String>,
    blur: Option<BlurSpec>,
) -> Option<DetailPair> {
    match state.runtime.spawn_blocking(move || compose_hero_pair(&paths, blur)).await {
        Ok(pair) => Some(pair),
        Err(e) => {
            log::warn!("curated hero compose: {}", describe(&e));
            None
        }
    }
}

/// Composes up to four covers into one banner artwork. Blocking. No usable source gives an empty
/// pair, which the publisher spends as no artwork rather than as a failure.
fn compose_hero_pair(paths: &[String], blur: Option<BlurSpec>) -> DetailPair {
    let sources: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    // `COVER_SIZE`, the tile `pair_from_image` reduces it to; a larger canvas reaches no surface.
    let Some(canvas) = compose_cover(&sources, COVER_SIZE) else {
        return DetailPair::default();
    };
    pair_from_image(&image::DynamicImage::ImageRgb8(canvas), blur)
        .map_or_else(DetailPair::default, Into::into)
}

/// The covers whose collage is on screen, so an unchanged top-4 costs no recompose.
///
/// `None` is nothing claimed, distinct from an empty set claimed, which still has to publish.
/// [`claim`](Self::claim) belongs past the section check: a set claimed ahead of a publish that
/// bails leaves every later refresh of those covers early-returning.
#[derive(Default)]
pub(crate) struct MosaicGuard(Mutex<Option<Vec<String>>>);

impl MosaicGuard {
    /// Whether `paths` differ from the last claim, the cheap skip ahead of a compose.
    pub(crate) fn is_stale(&self, paths: &[String]) -> bool {
        self.0.lock().as_deref() != Some(paths)
    }

    /// Claims `paths` as on screen; `false` when already claimed, so of two composes of one top-4
    /// in flight only the first paints.
    #[must_use]
    pub(crate) fn claim(&self, paths: Vec<String>) -> bool {
        let mut last = self.0.lock();
        if last.as_deref() == Some(paths.as_slice()) {
            return false;
        }
        *last = Some(paths);
        true
    }

    /// Forgets what is painted, so the next enter recomposes. Unconditional on the leave, since
    /// `release_section_state` bails once the user is already back.
    pub(crate) fn forget(&self) {
        *self.0.lock() = None;
    }
}

#[cfg(test)]
#[path = "tests/mosaic_hero_tests.rs"]
mod tests;
