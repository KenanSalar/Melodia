//! Publishes the hero band's solved colour set into the `HeroBackdrop` global.
//!
//! The thin half of [`crate::ui::backdrop`]: that module argues the colours and does
//! the maths, this one takes the measurement its caller decoded off the cover and
//! writes the answer. Six views share one global, so every hero opens by calling
//! exactly one of these.
//!
//! **Every hero has washes**, and they are the artwork's own wherever there is artwork: a genre
//! substitutes its name-hashed pair through [`apply_gradient`], and anything else with no cover
//! substitutes a seated accent pair inside [`aurora::tints`]. What the tiers over them are depends
//! on the arm — the blur solves them off the cover's hue, the aurora takes the theme's own — and
//! either way the set is a snapshot of the palette that was live when the hero opened, which is
//! what [`republish_for_palette`] exists to refresh.
//!
//! The whole set is published — scrim, gradient floor, the aurora's three washes,
//! hue-carrying chrome and both text tiers — so a hero and the Now Playing view answer
//! identically whichever backdrop the band is painting.

use std::cell::Cell;

use slint::ComponentHandle;

use crate::themes::color;
use crate::ui::aurora::{self, Tint, WASH_COUNT};
use crate::ui::backdrop::{self, BackdropColors, BackdropSample};
use crate::{AppWindow, HeroBackdrop};

thread_local! {
    /// What the set now in `HeroBackdrop` was derived from, so a palette change can re-solve it.
    /// `None` until the first hero opens.
    ///
    /// A thread-local because it shadows a global that is itself process-wide, and both are the
    /// UI thread's alone.
    static PUBLISHED_HERO: Cell<Option<PublishedHero>> = const { Cell::new(None) };
}

/// The two inputs a hero can be published from, kept so [`republish_for_palette`] can re-run
/// whichever one it was. **A genre belongs here too now**: its tiers were theme-independent while
/// it was permanently on the blur, and the aurora arm hands it `Theme.base` and a neutral ink.
#[derive(Clone, Copy)]
enum PublishedHero {
    Artwork(BackdropSample),
    Genre(GenreStops),
}

/// Genre Detail's two name-hashed pairs, from [`crate::ui::genres::genre_accent`].
///
/// Two, because the arms want different ones: the blur paints `floor` verbatim — the dimmed pair,
/// picked so the scrim it solves leaves the foreground legible — where the aurora washes `wash`,
/// the saturated pair the genre's own square and grid card paint, having no scrim for the dimming
/// to survive. A struct rather than four arguments, two same-typed pairs being swappable in
/// silence.
#[derive(Clone, Copy)]
pub(crate) struct GenreStops {
    pub floor: (u32, u32),
    pub wash: (u32, u32),
}

/// Which stops the gradient floor takes.
#[derive(Clone, Copy)]
enum Floor {
    /// The tier set's own — every hero but one.
    FromTiers,
    /// Genre Detail's dimmed pair, which only the blur arm ever paints: the aurora's floor is flat
    /// `Theme.base` like every other hero's, the genre's colours reaching it through the washes.
    Own(u32, u32),
}

/// Solve and publish from a cover's measurement. An empty sample — no artwork, or a
/// failed decode — takes the same path as every cover; what it falls back to on each arm, and why
/// that isn't a guess, is on [`BackdropSample::solve`] and [`aurora::tints`].
pub(crate) fn apply(ui: &AppWindow, sample: BackdropSample) {
    // One read for both halves: the tier set, and the seed the washes fall back to.
    let theme = backdrop::theme_tokens(ui);
    let colors = sample.solve(&theme, backdrop::kind(ui));
    let tints = aurora::tints(sample.seeds, &theme);
    PUBLISHED_HERO.set(Some(PublishedHero::Artwork(sample)));
    write(ui, &colors, &tints, Floor::FromTiers);
}

/// Re-solve the open hero against a palette that has just changed.
///
/// Every tier here is derived from the live theme — the aurora's verbatim, the blur's only where a
/// missing cover falls back to `Theme.accent` — and nothing else republishes: a hero is written at
/// open time and holds until the next one. A new accent would otherwise reach the band only on the
/// next drill, and the ink over an open hero would keep the *old* theme's tones against the base
/// the new one paints.
pub(crate) fn republish_for_palette(ui: &AppWindow) {
    match PUBLISHED_HERO.get() {
        Some(PublishedHero::Artwork(sample)) => apply(ui, sample),
        // Idempotent on the blur, whose stops are the genre's own — no second gate for an arm
        // that re-solves to the colours it already published.
        Some(PublishedHero::Genre(stops)) => apply_gradient(ui, stops),
        None => {}
    }
}

/// Solve and publish for the one hero with colours of its own rather than a cover — Genre Detail,
/// whose name-hashed stops come from [`crate::ui::genres::genre_accent`].
///
/// The washes are the saturated pair whichever arm runs — two opaque colours a quantizer could have
/// answered with, so [`aurora::tints`] fans a third off them and needs no genre case. What the arms
/// disagree about is the floor and the tiers over it, and the branch is here rather than at the call
/// site because [`backdrop::kind`] is the only place allowed to ask which surface is mounted:
///
/// - **Blur** — the dimmed pair verbatim as the floor, `stops.floor.0` doubling as the hue seed so
///   the chrome tier stays recognisably that genre's rather than reverting to the theme accent.
/// - **Aurora** — the theme's own tiers over a flat base, exactly as a cover gets.
pub(crate) fn apply_gradient(ui: &AppWindow, stops: GenreStops) {
    let theme = backdrop::theme_tokens(ui);
    let tints = aurora::tints([Some(stops.wash.0), Some(stops.wash.1), None, None], &theme);
    PUBLISHED_HERO.set(Some(PublishedHero::Genre(stops)));

    match backdrop::kind(ui) {
        backdrop::BackdropKind::Aurora => {
            write(ui, &backdrop::theme_backdrop(&theme), &tints, Floor::FromTiers);
        }
        backdrop::BackdropKind::Blur => {
            let luma = backdrop::gradient_luma(stops.floor.0, stops.floor.1);
            let colors = backdrop::solve(stops.floor.0, luma);
            write(ui, &colors, &tints, Floor::Own(stops.floor.0, stops.floor.1));
        }
    }
}

/// Reset to the floor solve on hero teardown, so backing out of one detail and into
/// another can't flash the previous entity's colours while the new cover decodes.
pub(crate) fn reset(ui: &AppWindow) {
    apply(ui, BackdropSample::default());
}

fn write(ui: &AppWindow, colors: &BackdropColors, tints: &[Tint; WASH_COUNT], floor: Floor) {
    let g = ui.global::<HeroBackdrop>();
    g.set_chrome(backdrop::chrome_brush(colors));
    g.set_placeholder(backdrop::placeholder_brush(colors));
    g.set_chrome_text(backdrop::chrome_text_brush(colors));
    g.set_chip_fill(backdrop::chip_fill_brush(colors));
    g.set_scrim(backdrop::scrim_brush(colors));
    g.set_on_backdrop(backdrop::text_brush(colors));
    g.set_on_backdrop_muted(backdrop::muted_brush(colors));

    let (floor_start, floor_end) = match floor {
        Floor::FromTiers => (colors.floor_start, colors.floor_end),
        Floor::Own(start_rgb, end_rgb) => (start_rgb, end_rgb),
    };
    g.set_floor_start(color(floor_start));
    g.set_floor_end(color(floor_end));

    let [tint_1, tint_2, tint_3] = tints;
    g.set_tint_1(tint_1.to_color());
    g.set_tint_2(tint_2.to_color());
    g.set_tint_3(tint_3.to_color());
}

#[cfg(test)]
#[path = "tests/hero_backdrop_tests.rs"]
mod tests;
