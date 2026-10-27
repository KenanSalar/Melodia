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

/// Pushes a decoded `(cover, blur)` pair into a hero global from the UI thread, the blur through
/// `write_crossfade_slot` so switching entities fades rather than flashes. `animate: true` is the
/// fresh open, `false` the watcher's refresh.
///
/// **`section_active` gates the `HeroBackdrop` write and nothing else**: the per-view slots are
/// this view's own and leave a hidden page ready to paint, but `HeroBackdrop` is one global for
/// six heroes, and a hidden view publishing into it paints its colours under whichever hero is on
/// screen. Pass the section's synchronous shadow, never a literal: the boot fetches every
/// persisted detail whichever section it restores.
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
/// Arcs release and `FemtoVG` can reclaim the GPU textures. Reach for
/// [`release_detail_hero_images`] unless the caller runs the shared resets on its own terms.
pub fn release_hero_slots(g: &impl HeroSlots) {
    g.write_cover(Image::default());
    g.write_blur_a(Image::default());
    g.write_blur_b(Image::default());
    g.write_has_blur(false);
}

/// The two shared resets every hero teardown owes: six heroes share one `HeroBackdrop` solve and
/// one `HeroChips` row, so a teardown leaving either paints the departing hero's colours and
/// counts under the next. On its own for the heroes with no slots to hand back, Genre Detail's
/// gradient and the two mosaic pages.
///
/// **On My Library a section leave is not a teardown, and the two halves stop at different
/// points.** The colour set is the page's, so [`crate::ui::my_library::the_band_is_up`] holds it
/// until the page is left, the question [`apply_detail_artwork`] asks on the way in. The chip row
/// clears earlier, at [`crate::ui::hero_chips::clear_if_stale`]: a colour held across a hand-off
/// is the outgoing hero's tone, a count held across it its facts under the incoming title.
pub fn release_shared_hero(ui: &AppWindow) {
    if !crate::ui::my_library::the_band_is_up(ui) {
        crate::ui::hero_backdrop::reset(ui);
    }
    crate::ui::hero_chips::clear_if_stale(ui);
}

/// [`release_hero_slots`] for one detail global, plus [`release_shared_hero`].
///
/// **The slots ride the page-level gate the colour set does.** Nothing clears a detail id on a
/// tab leave, so picking that tab again morphs the banner back open ahead of the re-fetch, and an
/// emptied `cover` would paint `ArtworkImage`'s fallback glyph through it. Every real close
/// writes `-1` first, and `release_collapsed_hero` hands the slots back once the band has shrunk.
pub fn release_detail_hero_images(ui: &AppWindow, g: &impl HeroSlots) {
    if !crate::ui::my_library::the_band_is_up(ui) {
        release_hero_slots(g);
    }
    release_shared_hero(ui);
}

/// Generate a curated page's banner publish. A macro because the body reaches into the page's
/// own `$Ui` handle (its section shadow and its mosaic guard), which no shared trait carries.
macro_rules! impl_curated_hero_helpers {
    ($Global:ty, $Ui:ty) => {
        /// Publish a composed banner and claim it as the one on screen.
        ///
        /// **Gated whole, where a detail view fills its slots even while hidden**: a curated
        /// page's leave wipes its models and forgets the guard, so a claim made behind it would
        /// suppress the re-enter's recompose, and a compose finishing after a nav away would
        /// paint this page's solve under whichever hero mounted next.
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
    };
}

pub(crate) use impl_curated_hero_helpers;

/// A view's persisted sort as `(field, dir)`, else `default_field` ascending. Every detail
/// `open_*` reads it, so reopening an entity restores the last sort picked for its view type. It
/// reads the file each call, which a boot-time seed shouldn't:
/// [`crate::ui::callbacks::persisted_sort`] is that one.
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
