//! The logo tier the station cards draw from, and its lifecycle.
//!
//! **The one card tier that is not `ui::grid_prewarm::tier()`**, `station-card.slint` reading the
//! decoded extent for its layout (`.claude/rules/ui-patterns.md`'s Covers): released outright on
//! leave, generation rewound to `0`, decode size retuned only while empty. It is the only thing the
//! leave frees, so this file is the whole of Radio's `release_section_state`.

use slint::ComponentHandle;

use crate::ui::grid_prewarm;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::{AppWindow, Radio};

use super::RadioUi;

/// How many leading logos a landed page warms, about a screenful; the rest decode on scroll-in.
const PREWARM_AHEAD: usize = 24;

pub fn new_tier() -> CoverThumbs {
    CoverThumbs::with_config(
        grid_prewarm::GRID_COVER_FALLBACK,
        grid_prewarm::GRID_COVER_CAP_FALLBACK,
    )
}

/// Sizes the logo cache against the display, after `app.show()` and on every resize.
pub fn tune_cache_for_display(app: &AppWindow, radio_ui: &RadioUi) {
    let thumb_size = grid_prewarm::cover_size_for_content(app);
    radio_ui.covers.resize(grid_prewarm::cover_cap_for_window(app, thumb_size));

    // The decode size moves only over an empty tier: the card divides `cover.width` by the
    // published size, so the two agree only while no buffer from the old size is held.
    if !radio_ui.covers.is_empty() {
        return;
    }
    radio_ui.covers.set_thumb_size(thumb_size);
    app.global::<Radio>().set_logo_decode_size(i32::try_from(thumb_size).unwrap_or(i32::MAX));
}

/// Resolves one card's logo without decoding on the calling thread. At generation `0` the leave
/// emptied the tier, so it answers from the cache alone until [`prewarm`] announces; past it a miss
/// is scheduled, the card coming back on the bump that follows.
pub fn logo_cover(radio_ui: &RadioUi, artwork_path: &str, generation: i32) -> slint::Image {
    let path = grid_prewarm::nonempty_artwork_path(artwork_path);
    if generation == 0 {
        radio_ui.covers.get_cached_opt(path)
    } else {
        radio_ui.covers.get_or_schedule_opt(path)
    }
}

/// Decodes about a screenful of logos off the UI thread, returning whether the tier still holds
/// them, a leave inside the burst having handed them back.
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

/// Hands the tier back and rewinds the generation to `0`, so cards mounting on it ask cache-only.
pub fn release(ui: &AppWindow, radio_ui: &RadioUi) {
    radio_ui.covers.clear();
    ui.global::<Radio>().set_covers_generation(0);
}

/// Re-runs the mounted card bindings once a scheduled decode lands, **never moving off `0`**: a
/// batch landing after a leave cleared the tier would otherwise read as warm.
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

/// Decodes `paths` on the blocking pool, then runs `then` and announces the tier on the UI thread,
/// re-checking `section_active` there: a leave after the prewarm returned has handed the buffers
/// back. A `JoinError` reads as a released tier.
///
/// `then` is what the caller owes on that tick, run whether or not the tier survived.
pub async fn warm_and_announce(
    radio_ui: &std::sync::Arc<RadioUi>,
    weak: &slint::Weak<AppWindow>,
    paths: Vec<String>,
    then: impl FnOnce(&AppWindow) + Send + 'static,
) {
    let warming = radio_ui.clone();
    let warmed =
        tokio::task::spawn_blocking(move || prewarm(&warming, &paths)).await.unwrap_or(false);

    let announcing = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        then(&ui);
        if warmed && announcing.section_active() {
            announce_warm(&ui);
        }
    });
}
