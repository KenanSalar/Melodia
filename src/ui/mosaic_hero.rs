//! The two mosaic heroes' shared blur application.
//!
//! Favorites and Recently Played paint the same banner from different data: a
//! 2×2 atlas of up-to-4 covers, blurred, cross-faded into whichever slot is
//! idle, with `HeroBackdrop` solved off the same buffer. Everything about
//! *applying* that result is identical between them — including the guard
//! placement, which is subtle enough that `.claude/rules/ui-patterns.md`
//! argues it at length and which was maintained in two copies before this.
//!
//! A macro rather than a generic function, for the reason
//! [`crate::ui::detail_view::impl_detail_view_helpers`] is one: `Favorites`
//! and `RecentlyPlayed` are distinct Slint-generated global types that happen
//! to declare the same `blur-img-a`/`-b`, `blur-use-a` and `has-blur`
//! properties, so there is no trait to be generic over.

/// Generate a mosaic hero's `clear_hero_blur` + `apply_hero_blur` pair.
///
/// `$Global` is the view's Slint global, `$Ui` its Rust handle — which must
/// offer `section_active()` and `state().last_mosaic_paths`.
macro_rules! impl_mosaic_hero {
    ($Global:ty, $Ui:ty) => {
        /// Clear the hero blur (e.g. no covers) without wiping the previous
        /// slot, so an in-flight fade completes before the gradient floor
        /// takes over.
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
                // Same rule as `apply_hero_blur`: the guard records what is
                // painted, so it moves only past the check that decides
                // whether anything is. A clear dropped by that check leaves
                // the previous covers recorded, and the next refresh tries
                // again.
                ui_handle.state().last_mosaic_paths.lock().clear();
                // With no mosaic left, the gradient floor is the whole
                // backdrop — re-solve against it so the scrim and foreground
                // match what is actually about to be on screen.
                $crate::ui::hero_backdrop::reset(&ui);
                // `None` through `write_crossfade_slot` clears `has-blur`
                // without wiping the previous slot, so any in-flight fade-out
                // completes naturally before the gradient floor takes over.
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

        /// Publish the composed mosaic, and record it as the one on screen.
        /// Skipped outright once the section is no longer active:
        /// `HeroBackdrop` is shared by all six heroes, so a compose that
        /// finishes after the user has navigated away would paint this view's
        /// solve under whichever hero mounted next. Because the
        /// `last_mosaic_paths` record lives past that check rather than before
        /// the compose, a skip here leaves the next refresh free to try the
        /// same covers again — the leave handler's `forget_mosaic` is a
        /// convenience, not the only thing keeping the guard honest.
        ///
        /// Refreshes overlapping the compose window all get here with the same
        /// covers, since none of them had recorded yet; the losers have
        /// nothing to add but a redundant cross-fade of the same buffer. How
        /// reachable that is differs per view — see each invocation site.
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
                // The hue and brightness were measured off this very buffer on
                // the blocking pool, so the scrim lands in step with the blur
                // under it.
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
