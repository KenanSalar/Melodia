//! Shared detail-view helper macro.
//!
//! The four detail views — `AlbumDetail`, `ArtistDetail`, `GenreDetail`,
//! `PlaylistDetail` — are distinct Slint-generated global types, so the
//! two boilerplate helpers every detail module needs (`apply_detail_artwork`
//! to write the cover + hero-blur pair, `replace_tracks_model` to swap the
//! `tracks` `VecModel`) can't be plain generic functions. This macro
//! stamps the typed body once per module, mirroring the
//! `impl_track_list_column_state!` pattern in `track_list_view.rs`.

/// Generate the per-view detail helpers for a Slint detail global.
///
/// Two arms:
/// * `artwork $Global` — views with a cover / hero-blur header
///   (`AlbumDetail`, `ArtistDetail`, `PlaylistDetail`): emits both
///   `apply_detail_artwork` and `replace_tracks_model`.
/// * `no_artwork $Global` — the `GenreDetail` view (procedural, no
///   intrinsic image): emits `replace_tracks_model` only.
macro_rules! impl_detail_view_helpers {
    (artwork $Global:ty) => {
        impl_detail_view_helpers!(@tracks_model $Global);

        /// Push a freshly-decoded `(cover, blur)` pair into the detail
        /// global from the UI thread: write the cover slot directly and
        /// route the blur through `write_crossfade_slot` so switching
        /// entities fades the previous blur to the new one without
        /// flashing. `animate: true` is the fresh-open path (a user
        /// click); `false` is the watcher-driven refresh.
        ///
        /// The hero's colour set is solved from the measurement the decode
        /// took off that same blur, so the scrim can't fall out of step
        /// with the buffer it is darkening.
        fn apply_detail_artwork(
            ui: &$crate::AppWindow,
            g: &$Global,
            pair: $crate::ui::detail_artwork::DetailPair,
            animate: bool,
        ) {
            g.set_cover(
                pair.cover.map(slint::Image::from_rgb8).unwrap_or_default(),
            );
            $crate::ui::hero_backdrop::apply(ui, pair.sample);
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
        /// Swap the detail global's `tracks` `VecModel` contents in
        /// place, falling back to a fresh model if the downcast fails
        /// (never expected — the model is always installed as a
        /// `VecModel`).
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

/// Resolve a view's sort as `(field, dir)` display strings from the
/// persisted `view_sort[view_id]`, falling back to `default_field`
/// ascending on a fresh install. Used by every detail `open_*` (so
/// reopening any entity restores the last sort picked for that view type)
/// and by `boot::spawn_initial_tracks_fetch` for the Tracks cold fetch.
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
