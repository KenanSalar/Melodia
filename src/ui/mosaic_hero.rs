//! The two mosaic heroes' shared blur application.
//!
//! Favorites and Recently Played paint the same banner from different data: a 2×2 atlas of
//! up to four covers, blurred, cross-faded into whichever slot is idle, with
//! `HeroBackdrop` solved off the same buffer. Everything about *applying* it is identical,
//! guard placement included — `.claude/rules/ui-patterns.md` argues that at length.
//!
//! A macro rather than a generic function, for
//! [`crate::ui::detail_view::impl_detail_view_helpers`]'s reason: the two globals are
//! distinct generated types that happen to declare the same four properties.

/// Generate a mosaic hero's `clear_hero_blur` + `apply_hero_blur` pair. `$Ui` must offer
/// `section_active()` and `state().last_mosaic_paths`.
macro_rules! impl_mosaic_hero {
    ($Global:ty, $Ui:ty) => {
        /// Clear the hero blur without wiping the previous slot, so an in-flight fade
        /// completes before the gradient floor takes over.
        fn clear_hero_blur(ui_handle: &std::sync::Arc<$Ui>, weak: &slint::Weak<$crate::AppWindow>) {
            use slint::ComponentHandle as _;

            let ui_handle = ui_handle.clone();
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                if !ui_handle.section_active() {
                    return;
                }
                let g = ui.global::<$Global>();
                // As in `apply_hero_blur`: the guard records what is painted, so it moves
                // only past the check that decides whether anything is.
                ui_handle.state().last_mosaic_paths.lock().clear();
                // With no mosaic left the gradient floor is the whole backdrop.
                $crate::ui::hero_backdrop::reset(&ui);
                // `None` clears `has-blur` without wiping the previous slot, so an
                // in-flight fade-out completes before the floor takes over.
                $crate::ui::now_playing::write_crossfade_slot(
                    None,
                    true,
                    g.get_blur_use_a(),
                    |img| g.set_blur_img_a(img),
                    |img| g.set_blur_img_b(img),
                    |v| g.set_blur_use_a(v),
                    |v| g.set_has_blur(v),
                );
            });
        }

        /// Publish the composed mosaic and record it as the one on screen. Skipped
        /// outright once the section is inactive: `HeroBackdrop` is shared by all six
        /// heroes, so a compose finishing after a nav away would paint this view's solve
        /// under whichever hero mounted next. The `last_mosaic_paths` record lives *past*
        /// that check rather than before the compose, so a skip leaves the next refresh
        /// free to try the same covers again.
        fn apply_hero_blur(
            ui_handle: &std::sync::Arc<$Ui>,
            weak: &slint::Weak<$crate::AppWindow>,
            composed: Option<$crate::ui::mosaic_blur::MosaicBlur>,
            animate: bool,
            paths: Vec<String>,
        ) {
            use slint::ComponentHandle as _;

            let ui_handle = ui_handle.clone();
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                if !ui_handle.section_active() {
                    return;
                }
                {
                    let mut last = ui_handle.state().last_mosaic_paths.lock();
                    if *last == paths {
                        return;
                    }
                    *last = paths;
                }
                let g = ui.global::<$Global>();
                // Measured off this very buffer on the blocking pool, so the scrim
                // lands in step with the blur under it.
                let sample = composed.as_ref().map(|m| m.sample).unwrap_or_default();
                $crate::ui::hero_backdrop::apply(&ui, sample);
                let img = composed.map(|m| slint::Image::from_rgb8(m.blur));
                $crate::ui::now_playing::write_crossfade_slot(
                    img,
                    animate,
                    g.get_blur_use_a(),
                    |i| g.set_blur_img_a(i),
                    |i| g.set_blur_img_b(i),
                    |v| g.set_blur_use_a(v),
                    |v| g.set_has_blur(v),
                );
            });
        }
    };
}

pub(crate) use impl_mosaic_hero;
