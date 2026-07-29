//! Publishes the hero band's solved colour set into the `HeroBackdrop` global.
//!
//! The thin half of [`crate::ui::backdrop`]: that module argues the colours and
//! does the maths, this one measures whatever the caller has in hand and writes
//! the answer. Six views share one global — see `globals.slint` for why one is
//! enough — so every hero opens by calling exactly one of these.
//!
//! The seed is the live `Theme.accent`, so the hero carries the theme's *hue*;
//! every tone comes from the solve, which is what keeps the band equally dark
//! under Mocha, Latte and macOS Light. As in the Now Playing view, a theme or
//! accent change reaches an already-open hero only on its next open — brightness
//! is unaffected either way, since no tone here is theme-derived.
//!
//! Only the *backdrop* tiers are published: scrim, gradient floor, and the
//! hue-carrying chrome. The hero's two text tiers are fixed constants on the
//! global, because a pinned backdrop is precisely what makes one fixed light
//! foreground correct on every cover.

use slint::{ComponentHandle, Rgb8Pixel, SharedPixelBuffer};

use crate::themes::{brush, brush_to_rgb, brush_with_alpha, color};
use crate::ui::backdrop::{self, BackdropColors};
use crate::{AppWindow, HeroBackdrop, Theme as ThemeGlobal};

/// Solve and publish from a decoded blur. `None` — no artwork, or a decode that
/// failed — runs the same solve against the gradient floor rather than taking a
/// separate path, since both of the floor's stops are ours and its brightness is
/// therefore known rather than guessed.
pub fn apply(ui: &AppWindow, blur: Option<&SharedPixelBuffer<Rgb8Pixel>>) {
    let seed = theme_accent(ui);
    let luma = blur.and_then(backdrop::luma_p90).unwrap_or_else(backdrop::floor_luma);
    write(ui, &backdrop::solve(seed, luma), None);
}

/// Solve and publish for a hero whose backdrop *is* a gradient — Genre Detail,
/// which has no artwork by nature and paints the name-hashed stops from
/// [`crate::ui::genres::color`]. Those stops are already theme-independent, so
/// they are kept verbatim and only measured; the scrim and foreground are
/// solved against them exactly as they would be against a cover.
///
/// `start_rgb` doubles as the hue seed, so the chrome tier stays recognisably
/// that genre's rather than reverting to the theme accent every other hero
/// seeds from.
pub fn apply_gradient(ui: &AppWindow, start_rgb: u32, end_rgb: u32) {
    let colors = backdrop::solve(start_rgb, backdrop::gradient_luma(start_rgb, end_rgb));
    write(ui, &colors, Some((start_rgb, end_rgb)));
}

/// Reset to the floor solve on hero teardown, so backing out of one detail view
/// and into another can't flash the previous entity's colours while the new
/// blur is still decoding.
pub fn reset(ui: &AppWindow) {
    apply(ui, None);
}

fn theme_accent(ui: &AppWindow) -> u32 {
    brush_to_rgb(&ui.global::<ThemeGlobal>().get_accent())
}

/// `floor_override` keeps a caller-supplied gradient instead of the solved one.
fn write(ui: &AppWindow, colors: &BackdropColors, floor_override: Option<(u32, u32)>) {
    let (floor_start, floor_end) =
        floor_override.unwrap_or((colors.floor_start, colors.floor_end));

    // No text tiers here: `HeroBackdrop.on-backdrop` / `-muted` are fixed `out`
    // properties. Pinning the backdrop is what makes one light foreground
    // correct on every cover — see the note beside them in `globals.slint`.
    let g = ui.global::<HeroBackdrop>();
    g.set_floor_start(color(floor_start));
    g.set_floor_end(color(floor_end));
    g.set_chrome(brush(colors.chrome));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "solved alpha is clamped to 0..=1 by `backdrop::scrim_alpha`"
    )]
    let scrim_alpha = (colors.scrim_alpha * 255.0).round() as u8;
    g.set_scrim(brush_with_alpha(colors.scrim, scrim_alpha));
}
