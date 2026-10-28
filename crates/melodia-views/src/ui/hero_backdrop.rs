//! Publishes the hero band's solved colour set into the `HeroBackdrop` global, all at once so a
//! hero and the Now Playing view answer alike; [`crate::ui::backdrop`] argues the colours.
//!
//! Every hero has washes: the artwork's own, a genre's name-hashed pair ([`apply_gradient`]), or a
//! seated accent pair inside [`aurora::tints`]. A band with no hero has none, which is [`reset`]'s
//! whole difference from an art-less [`apply`].

use std::cell::Cell;

use slint::ComponentHandle;

use crate::ui::appearance::rgb::color;
use crate::ui::aurora::{self, Tint, WASH_COUNT};
use crate::ui::backdrop::{self, BackdropColors, BackdropSample};
use melodia_ui::{AppWindow, HeroBackdrop};

thread_local! {
    /// What the published set came from, so a palette change can re-solve it; `None` is the idle
    /// set.
    static PUBLISHED_HERO: Cell<Option<PublishedHero>> = const { Cell::new(None) };
}

/// What a hero was published from, for [`republish_for_palette`] to re-run.
#[derive(Clone, Copy)]
enum PublishedHero {
    Artwork(BackdropSample),
    Genre(GenreStops),
}

/// Genre Detail's two name-hashed pairs ([`crate::ui::genres::genre_accent`]). The blur paints the
/// dimmed `floor` its scrim leaves legible; the aurora washes the saturated `wash` the genre's
/// square and card paint. A struct, two same-typed pairs being swappable in silence.
#[derive(Clone, Copy)]
pub(crate) struct GenreStops {
    pub floor: (u32, u32),
    pub wash: (u32, u32),
}

/// Which stops the gradient floor takes.
#[derive(Clone, Copy)]
enum Floor {
    /// The tier set's own, every hero but one.
    FromTiers,
    /// Genre Detail's dimmed pair, painted on the blur arm only.
    Own(u32, u32),
}

/// Solves and publishes from a cover's measurement, an empty sample taking the same path.
pub(crate) fn apply(ui: &AppWindow, sample: BackdropSample) {
    let theme = backdrop::theme_tokens(ui);
    let colors = sample.solve(&theme, backdrop::kind(ui));
    let tints = aurora::tints(sample.seeds, &theme);
    PUBLISHED_HERO.set(Some(PublishedHero::Artwork(sample)));
    write(ui, &colors, &tints, Floor::FromTiers);
}

/// Re-solves the open hero against a palette that just changed; otherwise a new accent would wait
/// for the next drill, and the open hero's ink would keep the old theme's tones.
///
/// The idle arm seeds the globals at boot, `appearance::install` landing here before any hero.
pub(crate) fn republish_for_palette(ui: &AppWindow) {
    match PUBLISHED_HERO.get() {
        Some(PublishedHero::Artwork(sample)) => apply(ui, sample),
        // Idempotent on the blur, whose stops are the genre's own.
        Some(PublishedHero::Genre(stops)) => apply_gradient(ui, stops),
        None => reset(ui),
    }
}

/// Solves and publishes Genre Detail, whose colours are its own stops rather than a cover's.
///
/// The washes are the saturated pair on both arms, [`aurora::tints`] fanning a third off them. The
/// blur takes the dimmed pair as its floor and `floor.0` as its hue, so the chrome stays the
/// genre's; the aurora gets the theme's tiers, exactly as a cover does.
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

/// Publishes the idle set: no washes, over [`backdrop::idle_backdrop`]'s floor. Not [`apply`] with
/// an empty sample, which seats an accent the next surface hasn't earned.
pub(crate) fn reset(ui: &AppWindow) {
    let theme = backdrop::theme_tokens(ui);
    let colors = backdrop::idle_backdrop(&theme, backdrop::kind(ui));
    let tints = aurora::idle_tints(&theme);
    PUBLISHED_HERO.set(None);
    write(ui, &colors, &tints, Floor::FromTiers);
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
