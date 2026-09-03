//! Shared hero-header helper macro.
//!
//! Every global carrying a hero is a distinct Slint-generated type, so the helpers their
//! modules need — `apply_detail_artwork` for the cover and hero-blur pair,
//! `replace_tracks_model` for the `tracks` `VecModel`, and the curated pages'
//! `publish_hero_artwork` / `republish_chips` — can't be generic functions. This stamps the
//! typed body once per module, as `impl_track_list_column_state!` does.

/// Generate the per-view hero helpers for a Slint global.
///
/// `artwork $Global` is a detail view with a cover / hero-blur header and emits both;
/// `curated $Global, $Ui, $publish_chips` is one of the two curated pages, whose track model is
/// its own tabbed cache's and whose banner is a composed collage;
/// `no_artwork $Global` is the procedural `GenreDetail` and emits only the model swap;
/// `artwork_only $Global` is the station detail, which has a hero and a list of bare titles
/// rather than a `TrackList`, so the model swap has nothing to swap.
macro_rules! impl_detail_view_helpers {
    (artwork $Global:ty) => {
        impl_detail_view_helpers!(@tracks_model $Global);
        impl_detail_view_helpers!(@artwork $Global);
    };
    (curated $Global:ty, $Ui:ty, $publish_chips:path) => {
        impl_detail_view_helpers!(@artwork $Global);

        /// Publish a composed banner and claim it as the one on screen.
        ///
        /// **Gated whole, where a detail view fills its own slots even while hidden.** A curated
        /// page's leave wipes its models and forgets the guard, so slots written behind it have
        /// nothing to be ready for and their claim would suppress the re-enter's recompose. What
        /// the gate mainly protects is `HeroBackdrop`, shared by all six heroes: a compose finishing
        /// after a nav away would paint this page's solve under whichever hero mounted next.
        fn publish_hero_artwork(
            view: &std::sync::Arc<$Ui>,
            weak: &slint::Weak<$crate::AppWindow>,
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
                apply_detail_artwork(&ui, &ui.global::<$Global>(), pair, animate, true);
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
            weak: &slint::Weak<$crate::AppWindow>,
        ) {
            let view = view.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                $publish_chips(&ui, &view);
            });
        }
    };
    (@artwork $Global:ty) => {
        /// Push a decoded `(cover, blur)` pair into the detail global from the UI
        /// thread: the cover slot directly, the blur through `write_crossfade_slot` so
        /// switching entities fades rather than flashes. `animate: true` is the
        /// fresh-open path, `false` the watcher-driven refresh. The colour set comes off the
        /// measurement the decode took beside these two buffers, so the scrim can't fall out
        /// of step with the blur it is darkening.
        ///
        /// **`section_active` gates the `HeroBackdrop` write and nothing else.** The
        /// per-view properties either side of it are this view's own, and writing them
        /// while hidden is what leaves the page ready to paint — but `HeroBackdrop` is
        /// one global for six heroes, so publishing into it from a view that isn't on
        /// screen paints this entity's colours under whichever hero is. Pass the
        /// section's synchronous shadow, never a literal: the boot path fetches every
        /// persisted detail id whichever section is restored.
        fn apply_detail_artwork(
            ui: &$crate::AppWindow,
            g: &$Global,
            pair: $crate::ui::detail_artwork::DetailPair,
            animate: bool,
            section_active: bool,
        ) {
            g.set_cover(
                pair.cover.map(slint::Image::from_rgb8).unwrap_or_default(),
            );
            if section_active {
                $crate::ui::hero_backdrop::apply(ui, pair.sample);
            }
            $crate::ui::now_playing::write_crossfade_slot(
                pair.blur.map(slint::Image::from_rgb8),
                animate,
                g.get_blur_use_a(),
                |img| g.set_blur_img_a(img),
                |img| g.set_blur_img_b(img),
                |v| g.set_blur_use_a(v),
                |v| g.set_has_blur(v),
            );
        }
    };
    (no_artwork $Global:ty) => {
        impl_detail_view_helpers!(@tracks_model $Global);
    };
    (artwork_only $Global:ty) => {
        impl_detail_view_helpers!(@artwork $Global);
    };
    (@tracks_model $Global:ty) => {
        /// Swap the detail global's `tracks` `VecModel` contents in place, through the keyed
        /// diff so re-opening the entity already on screen patches rather than resetting.
        /// Falls back to a fresh model if the downcast fails — never expected, the model
        /// always being installed as a `VecModel`.
        ///
        /// `rows` must already carry the selection it should end up with; the diff compares
        /// whole rows, so a caller that stamps selection afterwards would have it skipped.
        fn replace_tracks_model(g: &$Global, rows: Vec<$crate::TrackListRow>) {
            use slint::Model as _;
            let model = g.get_tracks();
            if let Some(vm) =
                model.as_any().downcast_ref::<slint::VecModel<$crate::TrackListRow>>()
            {
                $crate::ui::model_diff::apply_rows_keyed(vm, rows, |r| r.id);
            } else {
                g.set_tracks(slint::ModelRc::new(slint::VecModel::from(rows)));
            }
        }
    };
}

pub(crate) use impl_detail_view_helpers;

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
    melodia_app::library::settings::get_view_sort(state, view_id).map_or_else(
        || (default_field.to_owned(), "asc".to_owned()),
        |s| (s.field, s.dir.as_str().to_owned()),
    )
}
