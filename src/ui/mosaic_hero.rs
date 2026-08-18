//! What the two curated heroes share.
//!
//! Favorites and Recently Played draw their banner from up to four covers where every
//! other hero draws it from one. Composing that collage **once** is what collapses the
//! difference: past [`compose_hero_pair`] each is an ordinary single-artwork hero and
//! reuses the detail path whole — `apply_detail_artwork`, `write_crossfade_slot`,
//! `release_hero_slots!`. What is left here is the composition and the guard deciding
//! whether to redo it.

use std::path::PathBuf;

use parking_lot::Mutex;

use crate::media::artwork::compose_cover;
use crate::state::AppState;
use crate::ui::artwork_cache::{BlurSpec, pair_from_image};
use crate::ui::detail_artwork::DetailPair;
use crate::ui::util::COVER_SIZE;

/// [`compose_hero_pair`] on the blocking pool.
///
/// `None` only when the task itself failed, and the caller returns rather than publishing:
/// an empty pair would blank the banner *and* claim the set, where leaving the
/// [`MosaicGuard`] unclaimed lets the next tick try the same covers again.
pub(crate) async fn compose_off_thread(
    state: &AppState,
    paths: Vec<String>,
    blur: Option<BlurSpec>,
) -> Option<DetailPair> {
    match state.runtime.spawn_blocking(move || compose_hero_pair(&paths, blur)).await {
        Ok(pair) => Some(pair),
        Err(e) => {
            log::warn!("curated hero compose: {e}");
            None
        }
    }
}

/// Compose up to four covers into one banner artwork, on the detail band's blur spec.
///
/// **Blocking** — the compose, the blur and the measurement are all CPU-bound. An empty
/// list or one source that won't decode gives an empty pair, which the publisher spends as
/// "no artwork" rather than as a failure.
fn compose_hero_pair(paths: &[String], blur: Option<BlurSpec>) -> DetailPair {
    let sources: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    // Composed at the size the collage is *kept* at rather than at the persisted thumbnail's:
    // `pair_from_image`'s first act is to reduce it to a `COVER_SIZE` tile, so a larger canvas
    // costs a second resize of every source and reaches no surface.
    let Some(canvas) = compose_cover(&sources, COVER_SIZE) else {
        return DetailPair::default();
    };
    pair_from_image(&image::DynamicImage::ImageRgb8(canvas), blur).into()
}

/// The covers whose collage is on screen, so an unchanged top-4 costs no recompose — or `None`
/// while nothing is claimed.
///
/// **"Nothing claimed" and "claimed nothing" are different answers, hence the `Option`.** A page
/// whose set is empty — no favourites, or none with artwork — still has to *publish* that, the
/// seated accent being its honest colour. Collapsed onto a bare `Vec` the two states were one, so
/// an empty set was never stale, both curated pages early-returned before composing, and the band
/// kept whatever the last teardown or the last hero had left in the shared globals.
///
/// **[`claim`](Self::claim) belongs past the section check, never beside the compose.**
/// The guard means "this collage is what's painted", so a set claimed ahead of a publish
/// that then bails leaves every later refresh of those same covers early-returning, and
/// the banner sits on the bare gradient floor until the section leave forgets it.
#[derive(Default)]
pub(crate) struct MosaicGuard(Mutex<Option<Vec<String>>>);

impl MosaicGuard {
    /// Whether `paths` differ from what the last publish claimed — the cheap skip, taken
    /// before spending the blocking pool on a compose. Nothing claimed is stale against
    /// anything, the empty set included.
    pub(crate) fn is_stale(&self, paths: &[String]) -> bool {
        self.0.lock().as_deref() != Some(paths)
    }

    /// Claim `paths` as the collage on screen. `false` when the same set was already
    /// claimed: two composes of one top-4 can be in flight and only the first should paint.
    #[must_use]
    pub(crate) fn claim(&self, paths: Vec<String>) -> bool {
        let mut last = self.0.lock();
        if last.as_deref() == Some(paths.as_slice()) {
            return false;
        }
        *last = Some(paths);
        true
    }

    /// Forget what is painted, so the next enter recomposes. The section leave's, and
    /// unconditional there: `release_section_state` bails once the user is already back, so
    /// leaving the reset to it can strand the hero on the floor.
    pub(crate) fn forget(&self) {
        *self.0.lock() = None;
    }
}

#[cfg(test)]
#[path = "tests/mosaic_hero_tests.rs"]
mod tests;
