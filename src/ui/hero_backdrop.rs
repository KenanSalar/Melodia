//! Publishes the hero band's solved colour set into the `HeroBackdrop` global.
//!
//! The thin half of [`crate::ui::backdrop`]: that module argues the colours and does
//! the maths, this one takes the measurement its caller decoded off the blur and
//! writes the answer. Six views share one global, so every hero opens by calling
//! exactly one of these.
//!
//! The seed is the hue quantized out of the hero's own blur, so the band carries the
//! *artwork's* colour and changing the app accent leaves it where it is; `Theme.accent` is
//! the fallback for an entity with no hue to take. Every tone comes from the solve either
//! way, which is what keeps the band equally dark under every theme — so a theme change
//! reaches an already-open artwork-less hero only on its next open.
//!
//! The whole set is published — scrim, gradient floor, hue-carrying chrome and both text
//! tiers — so a hero and the Now Playing view solve identically.

use slint::ComponentHandle;

use crate::themes::{brush, brush_to_rgb, color};
use crate::ui::backdrop::{self, BackdropColors, BackdropSample};
use crate::{AppWindow, HeroBackdrop, Theme as ThemeGlobal};

/// Solve and publish from a blur's measurement. An empty sample — no artwork, or a
/// failed decode — takes the same path as every cover; what it falls back to, and why
/// that isn't a guess, is on [`BackdropSample::solve`].
pub(crate) fn apply(ui: &AppWindow, sample: BackdropSample) {
    write(ui, &sample.solve(theme_accent(ui)), None);
}

/// Solve and publish for a hero whose backdrop *is* a gradient — Genre Detail, which has
/// no artwork by nature and paints the name-hashed stops from
/// [`crate::ui::genres::genre_accent`]. Those are already theme-independent, so they are
/// kept verbatim and only measured.
///
/// `start_rgb` doubles as the hue seed, so the chrome tier stays recognisably that
/// genre's rather than reverting to the theme accent.
pub(crate) fn apply_gradient(ui: &AppWindow, start_rgb: u32, end_rgb: u32) {
    let colors = backdrop::solve(start_rgb, backdrop::gradient_luma(start_rgb, end_rgb));
    write(ui, &colors, Some((start_rgb, end_rgb)));
}

/// Reset to the floor solve on hero teardown, so backing out of one detail and into
/// another can't flash the previous entity's colours while the new blur decodes.
pub(crate) fn reset(ui: &AppWindow) {
    apply(ui, BackdropSample::default());
}

fn theme_accent(ui: &AppWindow) -> u32 {
    brush_to_rgb(&ui.global::<ThemeGlobal>().get_accent())
}

/// `floor_override` keeps a caller-supplied gradient instead of the solved one.
fn write(ui: &AppWindow, colors: &BackdropColors, floor_override: Option<(u32, u32)>) {
    let (floor_start, floor_end) = floor_override.unwrap_or((colors.floor_start, colors.floor_end));

    let g = ui.global::<HeroBackdrop>();
    g.set_floor_start(color(floor_start));
    g.set_floor_end(color(floor_end));
    g.set_chrome(brush(colors.chrome));
    g.set_scrim(backdrop::scrim_brush(colors));
    g.set_on_backdrop(brush(colors.text));
    g.set_on_backdrop_muted(brush(colors.muted));
}

#[cfg(test)]
#[path = "tests/hero_backdrop_tests.rs"]
mod tests;
