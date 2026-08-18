//! `sinks.view_model` subscriber + the (decode + metadata fetch + write)
//! apply step. Skipped while the view is closed; seeded on open.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, Weak};

use super::NowPlayingState;
use super::metadata::to_slint_track_meta;
use super::write_crossfade_slot;
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

/// Subscribe to `sinks.view_model` and react only to actual track changes. Always
/// stashes the current track; then, *only while the view is open*, decodes and blurs
/// the cover into the inactive slot, flips, and fetches the chips. Closed, it stashes
/// and stops — `wire_now_playing_open` decodes on the next open.
pub(super) fn spawn_track_change_subscriber(
    ui: &AppWindow,
    state: &AppState,
    np_artwork: Arc<NowPlayingArtwork>,
    np_state: Rc<NowPlayingState>,
    initial_track_id: Option<i64>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let state = state.clone();
    let mut rx = state.sinks.view_model.subscribe();
    // Seeded from the snapshot in `install` so the subscriber doesn't
    // re-fire for the already-seeded restored track.
    let mut last_track_id = initial_track_id;
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            // *Only* `current_track`: this fires on every `view_model` push — play,
            // pause, volume, speed — and acts solely on a track change, so cloning the
            // whole `ViewModel` is waste where this is an `Arc` refcount bump. The
            // borrow guard is statement-scoped, dropped before the `.await` below.
            let new_track = rx.borrow_and_update().as_ref().map(|vm| vm.current_track.clone());
            let Some(new_track) = new_track else { continue };
            let new_id = new_track.as_ref().map(|t| t.id);
            if new_id == last_track_id {
                continue;
            }
            last_track_id = new_id;
            // Regardless of visibility, so a later open can seed the artwork from it.
            np_state.current_track.borrow_mut().clone_from(&new_track);
            // The decode, blur and metadata read only produce something on screen while
            // a surface renders them — the full view, or the square miniplayer, whose
            // artwork reads the same dual slot. Otherwise `wire_now_playing_open` or
            // `kick_artwork` seeds on the next open.
            if np_state.open.get() || (np_state.mini_visible.get() && np_state.mini_square.get()) {
                apply_track_change(&weak, &state, &np_artwork, &np_state, new_track, true).await;
            }
        }
        log::debug!("ui::now_playing track-change subscriber stopped");
    }))?;
    Ok(())
}

/// Fetch metadata and decode the cover for `track`, then write both into the `Player`
/// global. Runs inside a `Compat` future, so it stays on the UI thread across the
/// `.await`s.
///
/// `animate` picks the cross-fade behaviour — `true` for a live change while the view is
/// open, `false` for the seed-on-open path. Records the applied id in
/// `applied_track_id`, so a redundant re-seed is skipped.
pub(super) async fn apply_track_change(
    weak: &Weak<AppWindow>,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &NowPlayingState,
    track: Option<Arc<TrackSummary>>,
    animate: bool,
) {
    let track_id = track.as_ref().map(|t| t.id);

    let meta = fetch_track_meta(state, track.as_ref()).await;
    let (cover, blurred, sample) = decode_artwork_for(state, np_artwork, track.as_ref()).await;

    // --- Write to Slint (UI thread) ---
    let Some(ui) = weak.upgrade() else { return };

    // A newer track change may have landed mid-decode: the open-seed task and the
    // subscriber can both have an `apply_track_change` in flight. If the decoded track
    // is no longer current, drop it — the newer call owns the slots and
    // `applied_track_id`.
    if track_id != np_state.current_track.borrow().as_ref().map(|t| t.id) {
        return;
    }

    let player = ui.global::<Player>();
    publish_chips(&player, np_state, meta);
    write_backdrop_tiers(&ui, sample);

    write_crossfade_slot(
        blurred,
        animate,
        player.get_blur_use_a(),
        |img| player.set_blur_img_a(img),
        |img| player.set_blur_img_b(img),
        |v| player.set_blur_use_a(v),
        |v| player.set_blur_has_image(v),
    );
    write_crossfade_slot(
        cover,
        animate,
        player.get_np_cover_use_a(),
        |img| player.set_np_cover_a(img),
        |img| player.set_np_cover_b(img),
        |v| player.set_np_cover_use_a(v),
        |v| player.set_np_cover_has_image(v),
    );

    np_state.applied_track_id.set(track_id);
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

/// Decode `track`'s cover on the blocking pool. A *single* decode derives both the sharp tile and
/// the blurred backdrop — the cover is the largest image on the app's hot path, so decoding it once
/// rather than twice halves the per-skip cost.
///
/// Every arm that isn't a decoded pair answers with an empty sample rather than a previous one:
/// `BackdropSample::solve` reads that as "no artwork" and falls back to `Theme.accent`, the honest
/// answer for a track whose cover is missing or unreadable.
async fn decode_artwork_for(
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    track: Option<&Arc<TrackSummary>>,
) -> DecodedArtwork {
    let empty = (None, None, BackdropSample::default());

    let Some(path) = track.and_then(|t| t.artwork_path.clone()).filter(|p| !p.is_empty()) else {
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
/// on every detail open, where all three callers of [`apply_track_change`] dedup on
/// `applied_track_id`, so without this the tiers hold until the next *track*.
pub(crate) fn republish_for_palette(ui: &AppWindow) {
    if let Some(sample) = PUBLISHED_SAMPLE.get() {
        write_backdrop_tiers(ui, sample);
    }
}
