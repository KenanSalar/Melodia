//! Source-level pins on the two backdrop stacks and the two sites that choose between them —
//! Now Playing, and `HeroBackdropStack` on behalf of the six heroes.
//!
//! The blur's floor is the layer that shipped without an `animate`, and it is the layer
//! that *is* the backdrop whenever the slots sit at 0: an art-less track, an artwork-less
//! entity, Genre Detail's stops, the window between a hero opening and its decode landing.
//! So the failure is invisible on anything with artwork. That reach is also why it takes a
//! gate, the globals behind it outliving the My Library tab that filled them.
//!
//! The pairing is pinned here rather than under a host, being a property of no single site.

// Comments dropped, so prose about a fix can neither satisfy a pin nor bound a region
// early — every anchor here is a gradient literal, and the block above the floor's own
// binding argues about gradients and stop counts.
use crate::test_support::{
    MIN_SLINT_SOURCES, UI_DIR, strip_line_comments as code, stripped_sources,
};

const HERO_BLUR: &str = include_str!("../../../melodia-ui/ui/components/hero-blur-backdrop.slint");

/// The two stacks, by the name a host mounts them under.
const STACKS: [&str; 2] = ["HeroBlurBackdrop", "AuroraBackdrop"];

/// The files that choose between them, and the only ones allowed to mount either.
///
/// Two, not three: the two shared bands had the same twenty-five lines each and now mount
/// `HeroBackdropStack`, which owns the pair on their behalf. Now Playing stays here because it
/// reads `Player.np-*` rather than the `HeroBackdrop` tier the wrapper is bound to — that split is
/// the whole reason the stacks take their colours as inputs.
const SITES: [&str; 2] = [
    "views/now-playing-view.slint",
    "components/hero/hero-backdrop-stack.slint",
];

/// The gradient floor eases, on the same token the layers above it take. Anchored on the
/// binding rather than searched loosely: what this catches is one line deleted from
/// directly under the gradient.
///
/// Safe to `animate` where a shared component's brush input is not — both sides are two
/// stops at 135deg and the pair has one writer, writing discretely, so nothing can restart
/// the binding mid-flight. `.claude/rules/slint-pitfalls.md` has the case where it can.
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

/// The stack defaults to shown, and every layer it paints drains with that gate.
///
/// The default is what a mount that passes nothing gets, and the other way round it comes up
/// transparent and stays there — on Genre Detail's, the one backdrop with no blur over it, that
/// is the whole banner. All three sites do pass it now, so this is the fourth mount's answer
/// rather than theirs.
///
/// **The two arms are checked one at a time**, an inverted ternary being the same failure
/// as an inverted default and reading correctly at a glance.
#[test]
fn the_stack_defaults_to_shown_and_drains_every_layer() {
    let code = code(HERO_BLUR);

    assert!(
        code.contains("in property <bool> shown: true;"),
        "`HeroBlurBackdrop.shown` must default to `true` — a mount that passes nothing has to \
         come up painting, not blank"
    );

    // At the ternary's own separators: neither arm is anything but a two-stop gradient,
    // so neither carries a `?` or a `:` and this lands exactly on the two halves.
    let arms = code
        .split_once("background: root.shown")
        .and_then(|(_, rest)| rest.split_once(";\n"))
        .map_or("", |(arms, _)| arms);
    assert!(
        !arms.is_empty(),
        "the floor no longer gates its gradient on `shown` — both pins below bound \
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
        code.contains("background: root.shown ? root.scrim : transparent;"),
        "the scrim drains with the floor — it was the one exempt layer while this stack was \
         alone on screen, and it now cross-fades against the aurora, where a scrim left at full \
         strength under a half-faded pair darkens the midpoint of every crossing"
    );
}

/// Neither stack names a global, which is what lets one component serve two deliberately
/// separate tiers — `Player.np-*` and `HeroBackdrop`. A single direct read ties the file
/// to whichever it names and puts the other site's inline copy back. `AuroraBackdrop`'s
/// half of this lives in `aurora_backdrop_tests`, with the rest of its pins.
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

/// Each site mounts both stacks once and hands each the negation of the other's `shown`.
///
/// **They are mounted together and cross-fade rather than swapped**, an `if` pair having nothing to
/// fade from: the loser goes transparent through its own `shown`, which is what the two components
/// spend their gated brushes on. So what a site can now get wrong is different from what it could
/// before — a stack missing its `shown` takes the `true` default and sits opaque over the other
/// forever, and two terms of the same sign leave the pair either blank or permanently blurred.
/// Both read correctly in the source and are only visible on the setting the author wasn't using.
///
/// The local `aurora-shown` property is still what keeps the condition single-sourced; the terms
/// are checked for the `!` rather than for equality with a whole string, since `LibraryTabBand`
/// folds its own `detail-open` into both.
#[test]
fn every_backdrop_site_cross_fades_between_the_two() {
    let tree = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    for site in SITES {
        let src = tree
            .iter()
            .find(|(path, _)| path.ends_with(site))
            .map(|(_, src)| src.as_str())
            .unwrap_or_default();
        assert!(
            src.contains("property <bool> aurora-shown:"),
            "{site} must name its choice once — both stacks read it, and a second spelling of \
             the condition can invert alone"
        );
        for (stack, sign) in STACKS.iter().zip(["!root.aurora-shown", "root.aurora-shown"]) {
            let mounts: Vec<&str> =
                src.lines().filter(|line| line.contains(&format!("{stack} {{"))).collect();
            assert_eq!(
                mounts.len(),
                1,
                "{site} must mount exactly one `{stack}` — it is the cross-fade's other half, \
                 and an unmounted stack has nothing to fade from"
            );
            // The mount is unconditional, which is the whole of the cross-fade: an `if` in front
            // of it reads exactly like the pair this replaced, still satisfies every `shown`
            // assertion below, and cuts.
            assert!(
                mounts.iter().all(|line| !line.contains("if ")),
                "{site} mounts `{stack}` behind an `if` — a branch destroys the stack outright, \
                 so there is nothing left to fade and the arms cut as they did before"
            );
            // `!root.aurora-shown` contains `root.aurora-shown`, so the blur's term is checked
            // first and the aurora's is checked for the negation's *absence*.
            let mount = src
                .split_once(&format!("{stack} {{"))
                .and_then(|(_, rest)| rest.split_once("\n    }"))
                .map(|(mount, _)| mount)
                .unwrap_or_default();
            let has_term =
                mount.contains(&format!("shown: {sign}")) || mount.contains(&format!("&& {sign}"));
            assert!(
                has_term,
                "{site}'s `{stack}` must take `shown:` carrying `{sign}` — without it the stack \
                 defaults to `true` and sits over the other one for good"
            );
        }
    }
}

/// No site gates the aurora on anything but the setting.
///
/// Each used to AND in a `has-tints` twin, back when an entry with no cover had no washes to lay;
/// `aurora::tints` substitutes the accent as its seed now, so every hero has an aurora and a
/// surviving gate would strand one site on the blur under a setting the other two honour.
#[test]
fn no_backdrop_site_gates_the_aurora_on_anything_but_the_setting() {
    let tree = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    for site in SITES {
        let src = tree
            .iter()
            .find(|(path, _)| path.ends_with(site))
            .map(|(_, src)| src.as_str())
            .unwrap_or_default();
        let binding = src
            .split_once("property <bool> aurora-shown:")
            .and_then(|(_, rest)| rest.split_once(';'))
            .map(|(binding, _)| binding)
            .unwrap_or_default();
        assert_eq!(
            binding.trim(),
            "Theme.aurora-backdrop",
            "{site} must read the setting and nothing else — a second term leaves this one site \
             painting the blur where its siblings paint the aurora"
        );
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
