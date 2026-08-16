//! Source-level pins on the two backdrop stacks and the three sites that choose between
//! them — `components/hero-blur-backdrop.slint`, `components/aurora-backdrop.slint`, and
//! Now Playing plus the two shared bands.
//!
//! The blur is a gradient floor, two cross-fading slots and a scrim solved against them,
//! all riding one curve. The floor is the layer that shipped without an `animate` — and
//! it is the layer that *is* the backdrop whenever the slots sit at 0: an art-less track,
//! an artwork-less entity, Genre Detail's name-hashed stops, the window between a hero
//! opening and its decode landing. So the failure is invisible on anything with artwork.
//!
//! That reach is also why the floor takes a gate: the layer that *is* the backdrop is the
//! one that eases, and on My Library the globals behind it outlive the tab that filled
//! them.
//!
//! The pairing is pinned here rather than under a host because it is a property of no
//! single site: three hosts, one stack each, and neither stack is allowed to know which
//! tier it is painting from.

// Comments dropped, so prose about a fix can neither satisfy a pin nor bound a region
// early — particularly here, where every anchor is a gradient literal and the block above
// the floor's own binding argues about gradients and stop counts.
use crate::test_support::{
    MIN_SLINT_SOURCES, UI_DIR, strip_line_comments as code, stripped_sources,
};

const HERO_BLUR: &str = include_str!("../../../melodia-ui/ui/components/hero-blur-backdrop.slint");

/// The two stacks, by the name a host mounts them under.
const STACKS: [&str; 2] = ["HeroBlurBackdrop", "AuroraBackdrop"];

/// The files that choose between them, and the only ones allowed to mount either.
const SITES: [&str; 3] = [
    "views/now-playing-view.slint",
    "components/hero/mosaic-tab-hero.slint",
    "components/hero/library-tab-band.slint",
];

/// The gradient floor eases, on the same token the layers above it take.
///
/// Anchored on the binding rather than searched loosely: what this exists to catch is one
/// line deleted from directly under the gradient.
///
/// Safe to `animate` where a shared component's brush input is not, which is the
/// distinction worth keeping straight: `Brush::interpolate` crosses gradients
/// stop-for-stop and both sides are two stops at 135deg, and each pair has exactly one
/// writer, writing discretely — so nothing can restart the binding mid-flight.
/// `.claude/rules/slint-pitfalls.md` has the case where that isn't true.
#[test]
fn the_backdrop_floor_eases_with_the_layers_above_it() {
    // On the gradient and then the end of its statement, rather than the line under
    // `background:` — the floor swaps its stops for a transparent pair while its host
    // paints no hero, so that binding spans three lines. What matters is the `animate`
    // directly under it, which both spellings still owe.
    let source = code(HERO_BLUR);
    let after = source
        .split_once("@linear-gradient(135deg,")
        .and_then(|(_, rest)| rest.split_once(";\n"))
        .map_or("", |(_, rest)| rest);
    assert_eq!(
        after.lines().next().unwrap_or_default().trim(),
        "animate background { duration: Theme.dur-med; easing: ease-in-out; }",
        "the gradient floor must ease on `dur-med` directly under its own binding — it is \
         the whole visible backdrop wherever the blur slots sit at 0, so a hard cut there \
         is the view stepping, and it reads as correct on every track or entity that has \
         artwork"
    );
}

/// `HeroBlurBackdrop` is nothing but the three layers, so its duration token can be
/// counted rather than located. A layer given a curve of its own makes a cover swap and
/// its scrim land a beat apart, which reads as a flicker rather than a wrong duration.
///
/// Counted over `code`, a count being the shape prose inflates most easily: one comment
/// quoting the animate block reads as a fifth layer.
#[test]
fn the_shared_backdrop_rides_one_duration() {
    assert_eq!(
        code(HERO_BLUR).matches("duration: Theme.dur-med").count(),
        4,
        "`HeroBlurBackdrop` is four animations on one token — both blur slots, the scrim \
         and the gradient floor"
    );
}

/// The floor's gate defaults to "a hero is up", which is the whole of what keeps the two
/// mosaic bands and Now Playing out of it: none of them ever stops painting one, so they
/// pass nothing and are unchanged. Only `LibraryTabBand` morphs, and only it can hold a
/// `HeroBackdrop` its band stopped painting a tab ago.
///
/// Defaulted the other way, every hero comes up on a transparent floor and stays there —
/// which on the one backdrop with no blur over it, Genre Detail's, is the whole banner.
/// **The two arms are checked one at a time**, an inverted ternary being the same failure
/// as an inverted default and reading correctly at a glance.
#[test]
fn the_floors_hero_gate_defaults_to_shown() {
    let code = code(HERO_BLUR);

    assert!(
        code.contains("in property <bool> hero-open: true;"),
        "`HeroBlurBackdrop.hero-open` must default to `true` — the mosaic bands and Now \
         Playing pass no value and always have a hero up, so the default is their whole \
         answer"
    );

    // At the ternary's own separators: neither arm is anything but a two-stop gradient,
    // so neither carries a `?` or a `:` and this lands exactly on the two halves.
    let arms = code
        .split_once("background: root.hero-open")
        .and_then(|(_, rest)| rest.split_once(";\n"))
        .map_or("", |(arms, _)| arms);
    assert!(
        !arms.is_empty(),
        "the floor no longer gates its gradient on `hero-open` — both pins below bound \
         against that ternary, and with nothing to split they report an inversion instead"
    );
    let (shown_arm, idle_arm) =
        arms.split_once('?').and_then(|(_, rest)| rest.split_once(':')).unwrap_or(("", ""));

    assert!(
        shown_arm.contains("root.floor-start") && shown_arm.contains("root.floor-end"),
        "the gated arm must be the solved floor — the other way round every hero comes up on \
         a transparent backdrop and stays there, which on Genre Detail is the whole banner"
    );
    assert!(
        idle_arm.contains("@linear-gradient(135deg, transparent, transparent)"),
        "the ungated arm must be a second two-stop gradient, not a bare colour — \
         `Brush::interpolate` crosses gradients stop-for-stop, and that is what lets an \
         opening hero ease up out of nothing instead of out of the last banner's stops"
    );

    assert!(
        code.contains("background: root.scrim;"),
        "the scrim stays ungated — it is solved to a tone that carries almost no chroma, so \
         it has nothing to flash, and draining it would brighten the artwork for the length \
         of every collapse"
    );
}

/// Neither stack names a global, which is what lets one component serve tiers that are
/// deliberately separate globals: `Player.np-*` for Now Playing, `HeroBackdrop` for the
/// six bands. A single direct read ties the file to whichever it names and puts the other
/// site's inline copy back. A `Theme.*` brush would be worse still — theme brushes flip
/// polarity with the variant while the foreground stays solved for dark.
///
/// `AuroraBackdrop` carries its own copy of this in `aurora_backdrop_tests`, where the
/// rest of its pins live; this is the half that stops the collapse regressing.
#[test]
fn the_blur_stack_names_no_global() {
    for global in [
        "Player.",
        "HeroBackdrop.",
        "Theme.base",
        "Theme.text",
        "Theme.accent",
    ] {
        assert!(
            !code(HERO_BLUR).contains(global),
            "`hero-blur-backdrop.slint` reads `{global}` directly — every colour is an input \
             so that one component can paint from either tier, and a global here is what \
             forced Now Playing to spell the whole stack inline"
        );
    }
}

/// Each site mounts exactly one of each stack, under one condition and its negation.
///
/// Slint has no element-level `else`, so the pair is two `if`s and the local property is
/// what keeps the condition single-sourced. Two regressions: a site that forgets an arm
/// (one setting paints nothing), and a site that keeps both mounted (the opaque one covers
/// the other, which is exactly how the aurora shipped before it became an arm).
#[test]
fn every_backdrop_site_mounts_one_of_the_two() {
    let tree = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    for site in SITES {
        let src = tree
            .iter()
            .find(|(path, _)| path.ends_with(site))
            .map(|(_, src)| src.as_str())
            .unwrap_or_default();
        assert!(
            src.contains("property <bool> aurora-shown:"),
            "{site} must name its choice once — two `if`s over one property, since Slint has \
             no element-level `else` and a second spelling of the condition can invert alone"
        );
        for (stack, arm) in STACKS.iter().zip(["if !root.aurora-shown:", "if root.aurora-shown:"]) {
            assert_eq!(
                src.matches(&format!("{arm} {stack} {{")).count(),
                1,
                "{site} must mount exactly one `{stack}` under `{arm}` — a missing arm leaves \
                 one setting painting nothing, and a second mount covers the arm being judged"
            );
        }
    }
}

/// Nowhere else mounts either stack. The pairing above is per-site, so it says nothing
/// about a fourth host that mounts one alone — which builds, looks right under whichever
/// setting it was written on, and paints the wrong backdrop under the other.
#[test]
fn no_other_view_mounts_a_backdrop_stack() {
    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        if SITES.iter().any(|site| path.ends_with(site)) {
            continue;
        }
        for stack in STACKS {
            assert!(
                // The mount, not the `import { … }` line or the `inherits` declaration.
                !src.contains(&format!("{stack} {{")),
                "{path} mounts `{stack}` — a backdrop is mounted only as one of a pair, and \
                 an unpaired one follows the setting on one arm and ignores it on the other"
            );
        }
    }
}
