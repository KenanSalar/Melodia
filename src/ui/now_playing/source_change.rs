//! `sinks.view_model` subscriber + the (decode + metadata fetch + write)
//! apply step. Skipped while the view is closed; seeded on open.
//!
//! **Source, not track.** A station has no `current_track` from the first tick to the last, so a
//! subscriber keyed on the track id never fires for one and the view paints an empty cover under
//! an empty title. What moves is `PlayerViewModelLight::source`, and everything below reduces to
//! the three things it answers: an identity to dedupe on, a path to decode, and an optional
//! `tracks` row for the chips.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, Weak};

use super::metadata::to_slint_track_meta;
use super::write_crossfade_slot;
use super::{NowPlayingSource, NowPlayingState, SourceKey};
use crate::entities::track::TrackSummary;
use crate::library;
use crate::state::AppState;
use crate::themes::color;
use crate::ui::aurora;
use crate::ui::backdrop::{self, BackdropSample};
use crate::ui::chips;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::{AppWindow, Player, TrackMetaRow};

thread_local! {
    /// The measurement behind whatever is in the `Player.np-*` tier now, so a palette change can
    /// re-solve it. `hero_backdrop::PUBLISHED_HERO`'s twin, `None` until the first track lands.
    static PUBLISHED_SAMPLE: Cell<Option<BackdropSample>> = const { Cell::new(None) };
}

/// What one artwork decode hands back to the UI thread: the sharp cover, the
/// blurred backdrop, and the hue + brightness measured off the sharp downscale.
type DecodedArtwork = (Option<Image>, Option<Image>, backdrop::BackdropSample);

/// Subscribe to `sinks.view_model` and react only to actual source changes. Always
/// stashes the current source; then, *only while the view is open*, decodes and blurs
/// the artwork into the inactive slot, flips, and fetches the chips. Closed, it stashes
/// and stops — `wire_now_playing_open` decodes on the next open.
pub(super) fn spawn_source_change_subscriber(
    ui: &AppWindow,
    state: &AppState,
    np_artwork: Arc<NowPlayingArtwork>,
    np_state: Rc<NowPlayingState>,
    initial_key: Option<SourceKey>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let state = state.clone();
    let mut rx = state.sinks.view_model.subscribe();
    // Seeded from the snapshot in `install` so the subscriber doesn't
    // re-fire for the already-seeded restored source.
    let mut last_key = initial_key;
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            // *Only* the source's identity: this fires on every `view_model` push — play,
            // pause, volume, speed, and every ICY title a station announces — and acts
            // solely on a source change. The borrow guard is statement-scoped, dropped
            // before the `.await` below.
            let new_source = rx.borrow_and_update().as_ref().map(NowPlayingSource::from_vm);
            let Some(new_source) = new_source else {
                continue;
            };
            let new_key = new_source.as_ref().map(|s| s.key.clone());
            if new_key == last_key {
                continue;
            }
            last_key = new_key;
            // Regardless of visibility, so a later open can seed the artwork from it.
            np_state.current_source.borrow_mut().clone_from(&new_source);
            // The decode, blur and metadata read only produce something on screen while
            // a surface renders them — the full view, or the square miniplayer, whose
            // artwork reads the same dual slot. Otherwise `wire_now_playing_open` or
            // `kick_artwork` seeds on the next open.
            if np_state.open.get() || (np_state.mini_visible.get() && np_state.mini_square.get()) {
                apply_source_change(&weak, &state, &np_artwork, &np_state, new_source, true).await;
            }
        }
        log::debug!("ui::now_playing source-change subscriber stopped");
    }))?;
    Ok(())
}

/// Fetch metadata and decode the cover for `track`, then write both into the `Player`
/// global. Runs inside a `Compat` future, so it stays on the UI thread across the
/// `.await`s.
///
/// `animate` picks the cross-fade behaviour — `true` for a live change while the view is
/// open, `false` for the seed-on-open path. Records the applied identity in
/// `applied_source`, so a redundant re-seed is skipped.
pub(super) async fn apply_source_change(
    weak: &Weak<AppWindow>,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &NowPlayingState,
    source: Option<NowPlayingSource>,
    animate: bool,
) {
    let key = source.as_ref().map(|s| s.key.clone());
    let track = source.as_ref().and_then(|s| s.track.clone());
    let artwork_path = source.as_ref().and_then(|s| s.artwork_path.clone());
    let is_station = matches!(key, Some(SourceKey::Station(_)));

    let meta = fetch_track_meta(state, track.as_ref()).await;
    let (cover, blurred, sample) = decode_artwork_for(state, np_artwork, artwork_path).await;

    // --- Write to Slint (UI thread) ---
    let Some(ui) = weak.upgrade() else { return };

    // A newer source change may have landed mid-decode: the open-seed task and the
    // subscriber can both have an `apply_source_change` in flight. If the decoded source
    // is no longer current, drop it — the newer call owns the slots and `applied_source`.
    if key != np_state.current_source.borrow().as_ref().map(|s| s.key.clone()) {
        return;
    }

    let player = ui.global::<Player>();
    publish_chips(&player, np_state, meta);
    write_backdrop_tiers(&ui, sample);

    // The blur is the backdrop for both kinds, so it takes the same cross-fade either way.
    write_crossfade_slot(
        blurred,
        animate,
        player.get_blur_use_a(),
        |img| player.set_blur_img_a(img),
        |img| player.set_blur_img_b(img),
        |v| player.set_blur_use_a(v),
        |v| player.set_blur_has_image(v),
    );

    // **The foreground splits by kind.** `np-cover-*` is the *track's* dual-slot cross-fade and a
    // station has nothing to cross-fade between; what a logo needs instead is its own pixel count,
    // so a favicon can draw as an inset card rather than be magnified across the tile. Whichever
    // slot isn't the live one is emptied, so a hand-off between kinds can't leave both painted.
    if is_station {
        let logo_size = cover.as_ref().map_or(0, native_size_of);
        player.set_np_station_logo(cover.unwrap_or_default());
        player.set_np_station_logo_size(logo_size);
        player.set_np_cover_has_image(false);
    } else {
        player.set_np_station_logo(slint::Image::default());
        player.set_np_station_logo_size(0);
        write_crossfade_slot(
            cover,
            animate,
            player.get_np_cover_use_a(),
            |img| player.set_np_cover_a(img),
            |img| player.set_np_cover_b(img),
            |v| player.set_np_cover_use_a(v),
            |v| player.set_np_cover_has_image(v),
        );
    }

    *np_state.applied_source.borrow_mut() = key;
}

/// A decoded image's own smallest side, which is what tells a tile whether it would have to
/// magnify. `pair_from_image` only ever shrinks, so this is the source's pixel count.
fn native_size_of(image: &Image) -> i32 {
    let size = image.size();
    i32::try_from(size.width.min(size.height)).unwrap_or(i32::MAX)
}

/// The eight chip columns for `track`, awaited inline — sqlx has a reactor here. Every failure arm
/// is the same empty row, which is what clears the chips: a missing row, a failed read and no track
/// at all are three spellings of "nothing to state about this track".
async fn fetch_track_meta(state: &AppState, track: Option<&Arc<TrackSummary>>) -> TrackMetaRow {
    let Some(track) = track else {
        return TrackMetaRow::default();
    };

    match library::tracks::get_track_meta(state, track.id).await {
        Ok(Some(meta)) => to_slint_track_meta(&meta),
        Ok(None) => TrackMetaRow::default(),
        Err(e) => {
            log::warn!("ui::now_playing get_track_meta({}): {e}", track.id);
            TrackMetaRow::default()
        }
    }
}

/// Decode the source's artwork on the blocking pool. A *single* decode derives both the sharp tile
/// and the blurred backdrop — the cover is the largest image on the app's hot path, so decoding it
/// once rather than twice halves the per-skip cost.
///
/// Takes a path rather than the source, which is all it ever wanted: `ArtworkCache` is path-keyed
/// and knows nothing about entity kinds, so a station's logo needs no tier of its own.
///
/// Every arm that isn't a decoded pair answers with an empty sample rather than a previous one:
/// `BackdropSample::solve` reads that as "no artwork" and falls back to `Theme.accent`, the honest
/// answer for a source whose artwork is missing or unreadable.
async fn decode_artwork_for(
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    artwork_path: Option<String>,
) -> DecodedArtwork {
    let empty = (None, None, BackdropSample::default());

    let Some(path) = artwork_path.filter(|p| !p.is_empty()) else {
        return empty;
    };

    let np = np_artwork.clone();
    match state.runtime.spawn_blocking(move || np.get_or_decode(Path::new(&path))).await {
        Ok(Some(pair)) => {
            (Some(Image::from_rgb8(pair.cover)), pair.blur.map(Image::from_rgb8), pair.sample)
        }
        Ok(None) => empty,
        Err(e) => {
            log::warn!("ui::now_playing artwork task join: {e}");
            empty
        }
    }
}

/// Chunk the new track's chips and push them with the meta row they came from.
///
/// Refreshes the shadow from the just-fetched `meta` and pushes a freshly chunked model, so the
/// view reflects the new track without waiting on a width-change fire. With no layout pass yet the
/// chunk collapses to one row and the strip's mount `Timer` fires a real width immediately. `None`
/// because this column can grow downward — the hero band can't.
fn publish_chips(player: &Player<'_>, np_state: &NowPlayingState, meta: TrackMetaRow) {
    let chip_texts = super::metadata::visible_chip_texts(&meta);
    let chip_rows = chips::chunk_chips_to_rows(&chip_texts, np_state.chip_last_width.get(), None);
    // Unconditional: a new track's chips are new text, which a row-length comparison
    // can't see. Recording the shape is what lets the width channel skip its repaints.
    *np_state.chip_last_shape.borrow_mut() = chips::split_shape(&chip_rows);
    player.set_chip_rows(chips::rows_to_model(chip_rows));
    *np_state.chip_texts.borrow_mut() = chip_texts;
    player.set_track_meta(meta);
}

/// Every colour this view paints on the backdrop, answered together. Which arm runs and both of
/// its fallbacks live on [`BackdropSample::solve`] and [`aurora::tints`], so this tier and the
/// hero's resolve them identically — including for a track with no artwork, which washes the
/// accent rather than dropping to the other backdrop.
fn write_backdrop_tiers(ui: &AppWindow, sample: BackdropSample) {
    let theme = backdrop::theme_tokens(ui);
    let colors = sample.solve(&theme, backdrop::kind(ui));
    let player = ui.global::<Player>();

    PUBLISHED_SAMPLE.set(Some(sample));

    player.set_np_accent_bright(backdrop::chrome_brush(&colors));
    player.set_np_chrome_text(backdrop::chrome_text_brush(&colors));
    player.set_np_chip_fill(backdrop::chip_fill_brush(&colors));
    player.set_np_viz(backdrop::viz_brush(&colors));
    player.set_np_on_backdrop(backdrop::text_brush(&colors));
    player.set_np_on_backdrop_muted(backdrop::muted_brush(&colors));
    player.set_np_floor_start(color(colors.floor_start));
    player.set_np_floor_end(color(colors.floor_end));
    player.set_np_scrim(backdrop::scrim_brush(&colors));

    let [tint_1, tint_2, tint_3] = aurora::tints(sample.seeds, &theme);
    player.set_np_tint_1(tint_1.to_color());
    player.set_np_tint_2(tint_2.to_color());
    player.set_np_tint_3(tint_3.to_color());
}

/// Re-solve the view's tiers against a palette that has just changed —
/// `hero_backdrop::republish_for_palette`'s twin, and the one that matters more. A band republishes
/// on every detail open, where all three callers of [`apply_source_change`] dedup on
/// `applied_source`, so without this the tiers hold until the next *source*.
pub(crate) fn republish_for_palette(ui: &AppWindow) {
    if let Some(sample) = PUBLISHED_SAMPLE.get() {
        write_backdrop_tiers(ui, sample);
    }
}
