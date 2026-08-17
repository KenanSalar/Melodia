//! Shared hero-header helper macro.
//!
//! Every global carrying a hero is a distinct Slint-generated type, so the two helpers
//! their modules need — `apply_detail_artwork` for the cover and hero-blur pair,
//! `replace_tracks_model` for the `tracks` `VecModel` — can't be generic functions. This
//! stamps the typed body once per module, as `impl_track_list_column_state!` does.

/// Generate the per-view hero helpers for a Slint global.
///
/// `artwork $Global` is a detail view with a cover / hero-blur header and emits both;
/// `artwork_only $Global` is a curated page, whose track model is its own tabbed cache's;
/// `no_artwork $Global` is the procedural `GenreDetail` and emits only the model swap.
macro_rules! impl_detail_view_helpers {
    (artwork $Global:ty) => {
        impl_detail_view_helpers!(@tracks_model $Global);
        impl_detail_view_helpers!(artwork_only $Global);
    };
    (artwork_only $Global:ty) => {
        /// Push a decoded `(cover, blur)` pair into the detail global from the UI
        /// thread: the cover slot directly, the blur through `write_crossfade_slot` so
        /// switching entities fades rather than flashes. `animate: true` is the
        /// fresh-open path, `false` the watcher-driven refresh. The colour set is
        /// solved from the measurement the decode took off that same blur, so the
        /// scrim can't fall out of step with the buffer it is darkening.
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
    (@tracks_model $Global:ty) => {
        /// Swap the detail global's `tracks` `VecModel` contents in place, falling back
        /// to a fresh model if the downcast fails — never expected, the model always
        /// being installed as a `VecModel`.
        fn replace_tracks_model(g: &$Global, rows: Vec<$crate::TrackListRow>) {
            use slint::Model as _;
            let model = g.get_tracks();
            if let Some(vm) =
                model.as_any().downcast_ref::<slint::VecModel<$crate::TrackListRow>>()
            {
                vm.set_vec(rows);
            } else {
                g.set_tracks(slint::ModelRc::new(slint::VecModel::from(rows)));
            }
        }
    };
}

pub(crate) use impl_detail_view_helpers;

/// A view's sort as `(field, dir)` display strings from the persisted
/// `view_sort[view_id]`, falling back to `default_field` ascending on a fresh install.
/// Every detail `open_*` uses it, so reopening any entity restores the last sort picked
/// for that view type, as does the Tracks cold fetch.
pub fn resolve_view_sort(
    state: &crate::state::AppState,
    view_id: &str,
    default_field: &str,
) -> (String, String) {
    crate::library::settings::get_view_sort(state, view_id).map_or_else(
        || (default_field.to_owned(), "asc".to_owned()),
        |s| (s.field, s.dir.as_str().to_owned()),
    )
}
