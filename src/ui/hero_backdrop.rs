//! Publishes the hero band's solved colour set into the `HeroBackdrop` global.
//!
//! The thin half of [`crate::ui::backdrop`]: that module argues the colours, this one writes the
//! answer. Six views share one global, so every hero opens by calling exactly one of these, and the
//! whole set goes out at once — scrim, floor, the three washes, chrome and both text tiers — so a
//! hero and the Now Playing view answer identically.
//!
//! **Every hero has washes**, the artwork's own wherever there is artwork: a genre substitutes its
//! name-hashed pair through [`apply_gradient`], anything else coverless a seated accent pair inside
//! [`aurora::tints`]. The set is a snapshot of the palette live when the hero opened, which is what
//! [`republish_for_palette`] refreshes.
//!
//! **A band with no hero on it has none**, and that is [`reset`]'s whole difference from an art-less
//! [`apply`]: it publishes [`aurora::idle_tints`] over [`backdrop::idle_backdrop`]'s floor, so a page
//! waiting on its collage paints the surface rather than a colour it has not earned yet.

use std::cell::Cell;

use slint::ComponentHandle;

use crate::ui::appearance::theme_apply::color;
use crate::ui::aurora::{self, Tint, WASH_COUNT};
use crate::ui::backdrop::{self, BackdropColors, BackdropSample};
use crate::{AppWindow, HeroBackdrop};

thread_local! {
    /// What the set now in `HeroBackdrop` was derived from, so a palette change can re-solve it.
    /// `None` is the idle set — before the first hero opens, and after every teardown.
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
/// picked so the scrim it solves leaves the foreground legible — where the aurora washes `wash`, the
/// saturated pair the genre's own square and grid card paint, having no scrim for the dimming to
/// survive. A struct rather than four arguments, two same-typed pairs being swappable in silence.
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

/// Solve and publish from a cover's measurement. An empty sample — no artwork, or a failed decode —
/// takes the same path as every cover; what it falls back to on each arm is on
/// [`BackdropSample::solve`] and [`aurora::tints`].
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
/// A hero is written at open time and holds until the next one, so without this a new accent would
/// reach the band only on the next drill and the ink over an open hero would keep the *old* theme's
/// tones against the base the new one paints.
///
/// **The idle arm is what seeds the globals at boot.** `appearance::install` applies the persisted
/// palette through `apply_palette`, which lands here before any hero has published — so a restored
/// curated page comes up on the theme's own base rather than `hero-backdrop.slint`'s placeholders.
pub(crate) fn republish_for_palette(ui: &AppWindow) {
    match PUBLISHED_HERO.get() {
        Some(PublishedHero::Artwork(sample)) => apply(ui, sample),
        // Idempotent on the blur, whose stops are the genre's own — no second gate for an arm
        // that re-solves to the colours it already published.
        Some(PublishedHero::Genre(stops)) => apply_gradient(ui, stops),
        None => reset(ui),
    }
}

/// Solve and publish for the one hero with colours of its own rather than a cover — Genre Detail,
/// whose name-hashed stops come from [`crate::ui::genres::genre_accent`].
///
/// The washes are the saturated pair whichever arm runs — two opaque colours a quantizer could have
/// answered with, so [`aurora::tints`] fans a third off them and needs no genre case. The arms
/// disagree only about the floor and the tiers over it:
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

/// Publish the idle set — no hero is painting, so the band takes the surface itself: **no washes at
/// all**, over [`backdrop::idle_backdrop`]'s floor.
///
/// Deliberately not [`apply`] with an empty sample. That is a hero that *has* opened with nothing to
/// quantize, and both its arms reach for the accent — a colour the next surface has not earned. A
/// curated banner wore it for the length of its collage compose.
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
