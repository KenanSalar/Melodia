//! Following the source on the deck into the view's artwork, backdrop and chips.
//!
//! **Source, not track**: a station has no `current_track` from first tick to last, so a subscriber
//! keyed on the track id never fires for one and the view paints an empty cover under an empty
//! title.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, ModelRc, VecModel, Weak};

use super::metadata::to_slint_track_meta;
use super::write_crossfade_slot;
use super::{NowPlayingSource, NowPlayingState, SourceKey};
use crate::ui::appearance::rgb::color;
use crate::ui::aurora;
use crate::ui::backdrop::{self, BackdropSample};
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Player, TrackMetaRow};

thread_local! {
    /// The measurement behind whatever is in the `Player.np-*` tier now, so a palette change can
    /// re-solve it. `hero_backdrop::PUBLISHED_HERO`'s twin, `None` until the first track lands.
    static PUBLISHED_SAMPLE: Cell<Option<BackdropSample>> = const { Cell::new(None) };
}

/// The cover, the blurred backdrop and the sample measured off the cover.
type DecodedArtwork = (Option<Image>, Option<Image>, backdrop::BackdropSample);

/// Stashes every source change, and applies it only while a surface draws the result.
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
    // So the restored source, already seeded, doesn't fire again.
    let mut last_key = initial_key;
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            // Fires on every push, a volume step or an ICY title included, so the identity is read
            // and compared under the borrow: a push that moved nothing allocates nothing.
            let new_source = {
                let vm = rx.borrow_and_update();
                let Some(vm) = vm.as_ref() else { continue };
                let source = vm.source();
                if SourceKey::describes(last_key.as_ref(), source.as_ref().map(|s| s.id)) {
                    continue;
                }
                NowPlayingSource::from_vm(vm)
            };
            last_key = new_source.as_ref().map(|s| s.key.clone());
            // Regardless of visibility, so a later open can seed the artwork from it.
            np_state.current_source.borrow_mut().clone_from(&new_source);
            // The logo is written on the far side of a decode, so until then it describes the
            // *outgoing* station under a name and facts that have already moved. Dropped the
            // moment it stops being true, which leaves the row tier drawing in the gap.
            if let Some(ui) = weak.upgrade() {
                let player = ui.global::<Player>();
                player.set_np_station_logo(Image::default());
                player.set_np_station_logo_size(0);
            }
            if np_state.renders_artwork() {
                apply_source_change(&weak, &state, &np_artwork, &np_state, new_source, true).await;
            }
        }
        log::debug!("ui::now_playing source-change subscriber stopped");
    }))?;
    Ok(())
}

/// Fetches the chips and decodes the artwork, then writes both and records the source in
/// `applied_source`. Animated for a live change, not for a seed.
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

    // Kicked, not awaited, and ahead of both reads: the lookup reaches a file and then a socket,
    // and awaited it would hold the cover and title behind a request that can time out. The hook
    // claims before its first `.await` and drops a result a newer song overtook, the guard below
    // one layer in.
    np_state.lyrics.kick();

    let meta = fetch_track_meta(state, track.as_ref()).await;
    let (cover, blurred, sample) = decode_artwork_for(state, np_artwork, artwork_path).await;

    let Some(ui) = weak.upgrade() else { return };

    // The open seed and the subscriber can both be mid-decode; the newer one owns the slots.
    if key != np_state.current_source.borrow().as_ref().map(|s| s.key.clone()) {
        return;
    }

    let player = ui.global::<Player>();
    publish_chips(&player, meta);
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

    // **The foreground splits by kind.** A station has nothing to cross-fade between, and a logo
    // needs its own pixel count instead, so a favicon draws as an inset card rather than magnified.
    // The slot that isn't live is emptied, so a hand-off between kinds can't leave both painted.
    if is_station {
        let logo_size = cover.as_ref().map_or(0, native_size_of);
        player.set_np_station_logo(cover.unwrap_or_default());
        player.set_np_station_logo_size(logo_size);
        player.set_np_cover_has_image(false);
        // The never-clear rule is for a track fading into another. Nothing fades into a station,
        // so the pair would hold a detail-tier decode, and its texture, for as long as it streams.
        player.set_np_cover_a(Image::default());
        player.set_np_cover_b(Image::default());
    } else {
        player.set_np_station_logo(Image::default());
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

/// Every failure arm is the empty row that clears the chips: a missing row, a failed read and no
/// track all mean nothing to state.
async fn fetch_track_meta(state: &AppState, track: Option<&Arc<TrackSummary>>) -> TrackMetaRow {
    let Some(track) = track else {
        return TrackMetaRow::default();
    };

    match library::tracks::get_track_meta(&state.db, track.id).await {
        Ok(Some(meta)) => to_slint_track_meta(&meta),
        Ok(None) => TrackMetaRow::default(),
        Err(e) => {
            log::warn!("ui::now_playing get_track_meta({}): {}", track.id, describe(&e));
            TrackMetaRow::default()
        }
    }
}

/// One decode yields both the sharp tile and the blur, the cover being the largest image on the hot
/// path. Path-keyed, so a station's logo needs no tier of its own. Every arm short of a decoded pair
/// answers an empty sample, which `BackdropSample::solve` reads as no artwork and washes the accent.
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
            log::warn!("ui::now_playing artwork task join: {}", describe(&e));
            empty
        }
    }
}

fn publish_chips(player: &Player<'_>, meta: TrackMetaRow) {
    let chips = super::metadata::visible_chip_texts(&meta);
    player.set_chips(ModelRc::from(Rc::new(VecModel::from(chips))));
    player.set_track_meta(meta);
}

/// Every colour this view paints on the backdrop. The arms and fallbacks are
/// [`BackdropSample::solve`]'s and [`aurora::tints`]', so this tier and the hero's resolve alike.
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

/// `hero_backdrop::republish_for_palette`'s twin. Every caller of [`apply_source_change`] dedupes
/// on `applied_source`, so without this the tiers would hold the old palette until the next source.
pub(crate) fn republish_for_palette(ui: &AppWindow) {
    if let Some(sample) = PUBLISHED_SAMPLE.get() {
        write_backdrop_tiers(ui, sample);
    }
}
