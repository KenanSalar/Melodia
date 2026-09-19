//! The logo tier the station cards draw from, and its lifecycle.
//!
//! **The one card tier that is not `ui::grid_prewarm::tier()`**, and the reason is the station
//! tile rather than the decode: `station-card.slint` derives `logo-native-size` from
//! `cover.width / logo-decode-size`, so the *decoded extent* is load-bearing for that card's
//! layout. The shared tier's leave shrinks what it holds to a proxy, which every other card reads
//! as the same picture and this one would read as a tiny source to inset. So this tier keeps the
//! old contract — released outright on leave, generation rewound to `0` — and a station's cold
//! card costs nothing anyway, its monogram and name-hashed tile being carried on the row.
//!
//! **It is also the only thing the leave frees.** The browse cache and its Slint model survive,
//! because re-entering the section must not cost a directory round trip; the decoded pixels are
//! where the bytes are. So this file is the whole of Radio's `release_section_state`.

use slint::ComponentHandle;

use crate::ui::grid_prewarm;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::{AppWindow, Radio};

use super::RadioUi;

/// How many leading logos a landed page warms before the grid paints. Roughly one screenful at any
/// reasonable column count; everything past it decodes lazily on scroll-in through `request-logo`.
const PREWARM_AHEAD: usize = 24;

pub fn new_tier() -> CoverThumbs {
    CoverThumbs::with_config(
        grid_prewarm::GRID_COVER_FALLBACK,
        grid_prewarm::GRID_COVER_CAP_FALLBACK,
    )
}

/// Size the logo cache against the display it will be drawn on. Called after `app.show()` and
/// again on every resize, off `WindowChrome.display-changed`.
pub fn tune_cache_for_display(app: &AppWindow, radio_ui: &RadioUi) {
    let cap = grid_prewarm::cover_cap_for_window(app);
    radio_ui.covers.resize(cap);

    // Published as well as set, because the card compares the logo it was handed against it to
    // decide whether the source can fill the tile — see `Radio.logo-decode-size`.
    let thumb_size = grid_prewarm::cover_size_for_window(app);
    radio_ui.covers.set_thumb_size(thumb_size);
    app.global::<Radio>().set_logo_decode_size(i32::try_from(thumb_size).unwrap_or(i32::MAX));
}

/// Resolve one station card's logo without ever decoding on the calling thread.
///
/// The branch every grid took before they shared a tier, kept here because this tier is still
/// released outright: `0` means the leave emptied it, so answer from the cache alone and let the
/// card fall back to its monogram until [`prewarm`] announces. Past `0` a miss is scheduled and
/// still answered with the monogram, the card coming back on the bump that follows.
pub fn logo_cover(radio_ui: &RadioUi, artwork_path: &str, generation: i32) -> slint::Image {
    let path = grid_prewarm::nonempty_artwork_path(artwork_path);
    if generation == 0 {
        radio_ui.covers.get_cached_opt(path)
    } else {
        radio_ui.covers.get_or_schedule_opt(path)
    }
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

/// Decode `paths` off the UI thread, then run `then` and announce the tier on the UI thread.
///
/// Both fill paths owe this exact pair. The decode goes to `spawn_blocking` because it is a
/// decode; the announce has to be back on the UI thread and has to re-check `section_active`,
/// since a leave landing after the prewarm returned has handed the buffers back, and bumping the
/// generation over an emptied tier is what the gate exists to stop.
/// A `JoinError` reads the same as a released tier: we do not know, so we do not announce.
///
/// `then` is whatever the caller owes on that same tick — Browse repaints its grid when a logo
/// landed, the kept tabs owe nothing — and runs whether or not the tier survived, being about the
/// rows rather than about the decode.
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
