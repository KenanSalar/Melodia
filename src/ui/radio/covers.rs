//! The logo tier the station cards draw from, and its lifecycle.
//!
//! Its own `CoverThumbs` rather than the shared row tier, for the reason every grid has one: a
//! card is drawn at grid size, and a leave has to be able to hand its buffers back without pulling
//! covers out from under the track lists.
//!
//! **The leave releases this and nothing else.** The browse cache and its Slint model survive,
//! because re-entering the section must not cost a directory round trip; what a leave actually
//! frees is the decoded pixels, which is where the bytes are. So this file is the whole of Radio's
//! `release_section_state`.

use std::num::NonZeroUsize;

use slint::ComponentHandle;

use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::grid_prewarm;
use crate::{AppWindow, Radio};

use super::RadioUi;

/// Fallback LRU capacity, used at construction and whenever the display can't be queried.
/// [`tune_cache_for_display`] replaces it once there is a window to measure. Must stay at or above
/// [`PREWARM_AHEAD`] or the prewarm would evict its own work.
const DEFAULT_LOGO_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("DEFAULT_LOGO_CAP > 0"),
};

/// How many leading logos a landed page warms before the grid paints. Roughly one screenful at any
/// reasonable column count; everything past it decodes lazily on scroll-in through `request-logo`.
const PREWARM_AHEAD: usize = 24;

pub fn new_tier() -> CoverThumbs {
    CoverThumbs::with_config(grid_prewarm::GRID_COVER_FALLBACK, DEFAULT_LOGO_CAP)
}

/// Size the logo cache against the display it will be drawn on. Called after `app.show()` and
/// again on every resize, off `WindowChrome.display-changed`.
pub fn tune_cache_for_display(app: &AppWindow, radio_ui: &RadioUi) {
    let cap = grid_prewarm::cover_cap_for_window(app, DEFAULT_LOGO_CAP);
    radio_ui.covers.resize(cap);

    // Published as well as set, because the card compares the logo it was handed against it to
    // decide whether the source can fill the tile — see `Radio.logo-decode-size`.
    let thumb_size = grid_prewarm::cover_size_for_window(app);
    radio_ui.covers.set_thumb_size(thumb_size);
    app.global::<Radio>().set_logo_decode_size(i32::try_from(thumb_size).unwrap_or(i32::MAX));
}

/// Decode roughly a screenful of logos, off the UI thread.
///
/// Returns whether the tier still holds what it warmed: a leave landing inside the burst hands the
/// buffers back, and announcing anyway would bump `covers-generation` over an emptied tier.
pub fn prewarm(radio_ui: &RadioUi, artwork_paths: &[String]) -> bool {
    let paths = grid_prewarm::unique_artwork_paths(
        artwork_paths.iter().map(|path| Some(path.as_str())),
        PREWARM_AHEAD,
    );
    if paths.is_empty() {
        return false;
    }
    radio_ui.covers.prewarm(&paths);
    radio_ui.section_active()
}

/// Hand the tier back and put the generation where a cold tier expects it.
///
/// The rewind to `0` is what makes the next mount ask cache-only, so cards mounting on a released
/// tier queue no decode for buffers this just dropped.
pub fn release(ui: &AppWindow, radio_ui: &RadioUi) {
    radio_ui.covers.clear();
    ui.global::<Radio>().set_covers_generation(0);
}

/// Re-run the mounted card bindings once a scheduled decode has landed, **never moving off 0** —
/// a batch landing after a leave cleared the tier would otherwise read as warm and cost the next
/// mount the cache-only frame the gate exists for.
pub fn repaint(ui: &AppWindow) {
    let g = ui.global::<Radio>();
    let generation = g.get_covers_generation();
    if generation > 0 {
        g.set_covers_generation(generation.saturating_add(1));
    }
}

/// Announce a warm tier, moving the generation off `0` for the first time.
pub fn announce_warm(ui: &AppWindow) {
    let g = ui.global::<Radio>();
    g.set_covers_generation(g.get_covers_generation().saturating_add(1));
}
