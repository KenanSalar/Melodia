//! What every hero banner shares: the slot set six globals carry, the write that fills it, the
//! releases that hand it back, and the curated pages' banner helpers.

use slint::Image;

use crate::ui::detail_artwork::DetailPair;
use melodia_ui::{
    AlbumDetail, AppWindow, ArtistDetail, Favorites, PlaylistDetail, Radio, RecentlyPlayed,
};

/// The cover and the two-slot blur cross-fade a hero global carries. Method names stay apart
/// from the generated accessors, as `RowSelectionView`'s do.
pub trait HeroSlots {
    fn write_cover(&self, cover: Image);
    fn blur_use_a(&self) -> bool;
    fn write_blur_a(&self, blur: Image);
    fn write_blur_b(&self, blur: Image);
    fn write_blur_use_a(&self, use_a: bool);
    fn write_has_blur(&self, has_blur: bool);
}

macro_rules! impl_hero_slots {
    ($($global:ident),+ $(,)?) => {$(
        impl HeroSlots for $global<'_> {
            fn write_cover(&self, cover: Image) {
                self.set_cover(cover);
            }
            fn blur_use_a(&self) -> bool {
                self.get_blur_use_a()
            }
            fn write_blur_a(&self, blur: Image) {
                self.set_blur_img_a(blur);
            }
            fn write_blur_b(&self, blur: Image) {
                self.set_blur_img_b(blur);
            }
            fn write_blur_use_a(&self, use_a: bool) {
                self.set_blur_use_a(use_a);
            }
            fn write_has_blur(&self, has_blur: bool) {
                self.set_has_blur(has_blur);
            }
        }
    )+};
}

impl_hero_slots!(AlbumDetail, ArtistDetail, PlaylistDetail, Favorites, RecentlyPlayed, Radio);

/// Push a decoded `(cover, blur)` pair into a hero global from the UI thread: the cover slot
/// directly, the blur through `write_crossfade_slot` so switching entities fades rather than
/// flashes. `animate: true` is the fresh-open path, `false` the watcher-driven refresh. The
/// colour set comes off the measurement the decode took beside these two buffers, so the scrim
/// can't fall out of step with the blur it is darkening.
///
/// **`section_active` gates the `HeroBackdrop` write and nothing else.** The per-view properties
/// either side of it are this view's own, and writing them while hidden is what leaves the page
/// ready to paint — but `HeroBackdrop` is one global for six heroes, so publishing into it from a
/// view that isn't on screen paints this entity's colours under whichever hero is. Pass the
/// section's synchronous shadow, never a literal: the boot path fetches every persisted detail id
/// whichever section is restored.
pub(crate) fn apply_detail_artwork(
    ui: &AppWindow,
    g: &impl HeroSlots,
    pair: DetailPair,
    animate: bool,
    section_active: bool,
) {
    g.write_cover(pair.cover.map(Image::from_rgb8).unwrap_or_default());
    if section_active {
        crate::ui::hero_backdrop::apply(ui, pair.sample);
    }
    crate::ui::now_playing::write_crossfade_slot(
        pair.blur.map(Image::from_rgb8),
        animate,
        g.blur_use_a(),
        |img| g.write_blur_a(img),
        |img| g.write_blur_b(img),
        |v| g.write_blur_use_a(v),
        |v| g.write_has_blur(v),
    );
}

/// Reset one hero global's image slots and clear `has-blur`, so the backing `SharedPixelBuffer`
/// Arcs release and `FemtoVG` can reclaim the GPU textures.
///
/// Reach for [`release_detail_hero_images`] unless you are handing back several globals at once
/// and want the two shared resets run once — My Library's deferred hero teardown is the only such
/// caller.
pub fn release_hero_slots(g: &impl HeroSlots) {
    g.write_cover(Image::default());
    g.write_blur_a(Image::default());
    g.write_blur_b(Image::default());
    g.write_has_blur(false);
}

/// The two shared resets every hero teardown owes, on their own.
///
/// Six heroes share one `HeroBackdrop` solve and one `HeroChips` row, so a teardown leaving either
/// behind paints the departing hero's colours and counts under the *next* one.
/// [`release_detail_hero_images`] is this plus the image slots; this bare pair is for the heroes
/// with none to hand back — Genre Detail's hashed gradient, and the two mosaic pages whose tiles
/// belong to their own tier.
///
/// **On My Library a section leave is not a teardown, and the two halves stop short of one at
/// different points.** The *colour set* belongs to the page rather than a tab, so
/// [`crate::ui::my_library::the_band_is_up`] holds it until the page itself is left — the same
/// question [`apply_detail_artwork`] asks on the way in, covering the two cases a hand-off gate
/// missed: the band *collapsing* over a hero whose colours it is still painting, and a tab
/// re-entered onto a detail whose banner comes back with it.
///
/// The *chip row* stops one step earlier, at [`crate::ui::hero_chips::clear_if_stale`], and the
/// asymmetry is the point: a colour held across a hand-off is the outgoing hero's *tone*, where a
/// count held across it is its *facts* under the incoming title.
pub fn release_shared_hero(ui: &AppWindow) {
    if !crate::ui::my_library::the_band_is_up(ui) {
        crate::ui::hero_backdrop::reset(ui);
    }
    crate::ui::hero_chips::clear_if_stale(ui);
}

/// [`release_hero_slots`] for one detail global, plus [`release_shared_hero`].
///
/// **The slots ride the same page-level gate the colour set does**, for the reason the detail id
/// gives: nothing clears one on a tab leave, so `AlbumDetail.album-id >= 0` still means "this
/// banner is in the globals" and picking that tab again morphs it back open ahead of the re-fetch —
/// emptying `cover` here leaves that morph painting `ArtworkImage`'s fallback glyph. Every path
/// that genuinely closes a detail writes `-1` first, and `release_collapsed_hero` hands the slots
/// back once the band has shrunk.
pub fn release_detail_hero_images(ui: &AppWindow, g: &impl HeroSlots) {
    if !crate::ui::my_library::the_band_is_up(ui) {
        release_hero_slots(g);
    }
    release_shared_hero(ui);
}

/// Generate a curated page's banner helpers.
///
/// A macro because both bodies reach into the page's own `$Ui` handle (its section shadow and
/// its mosaic guard) and name its chip publisher, which no trait between the two pages carries.
macro_rules! impl_curated_hero_helpers {
    ($Global:ty, $Ui:ty, $publish_chips:path) => {
        /// Publish a composed banner and claim it as the one on screen.
        ///
        /// **Gated whole, where a detail view fills its own slots even while hidden.** A curated
        /// page's leave wipes its models and forgets the guard, so slots written behind it have
        /// nothing to be ready for and their claim would suppress the re-enter's recompose. What
        /// the gate mainly protects is `HeroBackdrop`, shared by all six heroes: a compose finishing
        /// after a nav away would paint this page's solve under whichever hero mounted next.
        fn publish_hero_artwork(
            view: &std::sync::Arc<$Ui>,
            weak: &slint::Weak<melodia_ui::AppWindow>,
            pair: $crate::ui::detail_artwork::DetailPair,
            animate: bool,
            paths: Vec<String>,
        ) {
            let view = view.clone();
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                if !view.section_active() || !view.state().last_mosaic_paths.claim(paths) {
                    return;
                }
                $crate::ui::detail_view::apply_detail_artwork(
                    &ui,
                    &ui.global::<$Global>(),
                    pair,
                    animate,
                    true,
                );
            });
        }

        /// Re-publish the band's chips on the UI thread.
        ///
        /// **Call this wherever one of the chips' inputs lands.** Both curated pages assemble their
        /// band from more than one fetch and run those *concurrently*, so no ordering can be
        /// assumed; the publish reads only finished values, so the worst a mistimed one can be is a
        /// tick behind, never half-built. The grid path can't stand in for it — it publishes past a
        /// signature early-return, and `mounted_content` is a constant `0` on the Songs tab.
        pub fn republish_chips(
            view: &std::sync::Arc<$Ui>,
            weak: &slint::Weak<melodia_ui::AppWindow>,
        ) {
            let view = view.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                $publish_chips(&ui, &view);
            });
        }
    };
}

pub(crate) use impl_curated_hero_helpers;

/// A view's sort as `(field, dir)` display strings from the persisted
/// `view_sort[view_id]`, falling back to `default_field` ascending on a fresh install.
/// Every detail `open_*` uses it, so reopening any entity restores the last sort picked for that
/// view type. It reads the file each call, which is what those want and what a boot-time seed
/// does not: reach for [`crate::ui::callbacks::persisted_sort`] there instead.
pub fn resolve_view_sort(
    state: &melodia_app::state::AppState,
    view_id: &str,
    default_field: &str,
) -> (String, String) {
    melodia_app::library::settings::get_view_sort(&state.paths, view_id).map_or_else(
        || (default_field.to_owned(), "asc".to_owned()),
        |s| (s.field, s.dir.as_str().to_owned()),
    )
}
