//! Publishes the hero band's solved colour set into the `HeroBackdrop` global.
//!
//! The thin half of [`crate::ui::backdrop`]: that module argues the colours and does
//! the maths, this one takes the measurement its caller decoded off the cover and
//! writes the answer. Six views share one global, so every hero opens by calling
//! exactly one of these.
//!
//! The washes are the artwork's own colours whichever backdrop is mounted. What the tiers
//! over them are depends on the arm: the blur solves them off the cover's hue, the aurora
//! takes the theme's own. Either way the set is a snapshot of the palette that was live when the
//! hero opened, which is what [`republish_for_palette`] exists to refresh.
//!
//! **An entry with no artwork has no aurora to paint**, so it publishes `has-tints: false` and the
//! blur's own tiers, exactly as Genre Detail does — see [`BackdropSample::solve`] for why washing
//! the app's accent instead is the wrong answer.
//!
//! The whole set is published — scrim, gradient floor, the aurora's four washes,
//! hue-carrying chrome and both text tiers — so a hero and the Now Playing view answer
//! identically whichever backdrop the band is painting.

use std::cell::Cell;

use slint::ComponentHandle;

use crate::themes::{brush, color};
use crate::ui::aurora::{self, Tint, WASH_COUNT};
use crate::ui::backdrop::{self, BackdropColors, BackdropSample};
use crate::{AppWindow, HeroBackdrop};

thread_local! {
    /// The measurement behind whatever is in `HeroBackdrop` now, so a palette change can re-solve
    /// it. `None` for a genre — [`apply_gradient`]'s stops are theme-independent and re-solving
    /// them would only overwrite the banner on screen with the last *artwork* hero's.
    ///
    /// A thread-local because it shadows a global that is itself process-wide, and both are the
    /// UI thread's alone.
    static PUBLISHED_SAMPLE: Cell<Option<BackdropSample>> = const { Cell::new(None) };
}

/// What a hero's backdrop is derived from, and the one axis [`write`] branches on. A cover answers
/// with washes and a floor to lay them over; a genre has neither and keeps its own name-hashed
/// stops. One value rather than two optional arguments that could never disagree — and the
/// `Option` inside `Artwork` is the same argument one level down, `has-tints` being exactly
/// "are there washes" rather than a flag a caller could set against the colours it passed.
enum HeroFill {
    Artwork(Option<[Tint; WASH_COUNT]>),
    Gradient { start_rgb: u32, end_rgb: u32 },
}

/// Solve and publish from a cover's measurement. An empty sample — no artwork, or a
/// failed decode — takes the same path as every cover; what it falls back to, and why
/// that isn't a guess, is on [`BackdropSample::solve`].
///
/// **No washes without a cover to take them from**, so an entry showing the placeholder keeps the
/// blur whatever the setting says. That is one decision with two halves: `solve` already picks the
/// blur's tiers for an empty sample, and `has-tints` is what stops the mount painting the aurora
/// over them.
pub(crate) fn apply(ui: &AppWindow, sample: BackdropSample) {
    // One read for both halves: the tier set on the aurora arm, and the band the washes are
    // clamped into over it.
    let theme = backdrop::theme_tokens(ui);
    let colors = sample.solve(&theme, backdrop::kind(ui));
    let tints = sample.carries_artwork().then(|| aurora::tints(sample.seeds, &theme));
    PUBLISHED_SAMPLE.set(Some(sample));
    write(ui, &colors, HeroFill::Artwork(tints));
}

/// Re-solve the open hero against a palette that has just changed.
///
/// Every tier here is derived from the live theme — the aurora's verbatim, the blur's only where a
/// missing cover falls back to `Theme.accent` — and nothing else republishes: a hero is written at
/// open time and holds until the next one. A new accent would otherwise reach the band only on the
/// next drill, and the ink over an open hero would keep the *old* theme's tones against the base
/// the new one paints.
pub(crate) fn republish_for_palette(ui: &AppWindow) {
    if let Some(sample) = PUBLISHED_SAMPLE.get() {
        apply(ui, sample);
    }
}

/// Solve and publish for a hero whose backdrop *is* a gradient — Genre Detail, which has
/// no artwork by nature and paints the name-hashed stops from
/// [`crate::ui::genres::genre_accent`]. Those are already theme-independent, so they are
/// kept verbatim and only measured.
///
/// `start_rgb` doubles as the hue seed, so the chrome tier stays recognisably that
/// genre's rather than reverting to the theme accent.
///
/// **Reaches [`backdrop::solve`] directly, which is the blur's path, whatever the rest of
/// the app is painting.** The aurora washes a record's own colours over the theme's base; a
/// genre has no record, and these stops are only legible under the scrim that arm solves.
pub(crate) fn apply_gradient(ui: &AppWindow, start_rgb: u32, end_rgb: u32) {
    let luma = backdrop::gradient_luma(start_rgb, end_rgb);
    let colors = backdrop::solve(start_rgb, luma);
    PUBLISHED_SAMPLE.set(None);
    write(ui, &colors, HeroFill::Gradient { start_rgb, end_rgb });
}

/// Reset to the floor solve on hero teardown, so backing out of one detail and into
/// another can't flash the previous entity's colours while the new cover decodes.
pub(crate) fn reset(ui: &AppWindow) {
    apply(ui, BackdropSample::default());
}

fn write(ui: &AppWindow, colors: &BackdropColors, fill: HeroFill) {
    let g = ui.global::<HeroBackdrop>();
    g.set_chrome(backdrop::chrome_brush(colors));
    g.set_scrim(backdrop::scrim_brush(colors));
    g.set_on_backdrop(brush(colors.text));
    g.set_on_backdrop_muted(brush(colors.muted));

    match fill {
        // A `None` leaves the washes where they are, as the gradient arm does: nothing paints them
        // while `has-tints` is false, and the floor is the blur's own either way.
        HeroFill::Artwork(tints) => {
            g.set_floor_start(color(colors.floor_start));
            g.set_floor_end(color(colors.floor_end));
            g.set_has_tints(tints.is_some());
            if let Some([tint_1, tint_2, tint_3]) = tints {
                g.set_tint_1(tint_1.to_color());
                g.set_tint_2(tint_2.to_color());
                g.set_tint_3(tint_3.to_color());
            }
        }
        HeroFill::Gradient { start_rgb, end_rgb } => {
            g.set_floor_start(color(start_rgb));
            g.set_floor_end(color(end_rgb));
            g.set_has_tints(false);
        }
    }
}

#[cfg(test)]
#[path = "tests/hero_backdrop_tests.rs"]
mod tests;
