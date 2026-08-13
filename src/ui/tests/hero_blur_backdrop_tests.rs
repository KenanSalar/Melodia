//! Source-level pins on the two backdrop stacks — `components/hero-blur-backdrop.slint`,
//! which both shared bands mount, and the Now Playing view's own copy of the same three
//! layers.
//!
//! A gradient floor, two cross-fading blur slots, and a scrim solved against them. All
//! three have to ride one curve, and the floor is the layer that shipped without an
//! `animate` — in both files, because they were written apart. It is also the layer that
//! *is* the backdrop whenever the slots sit at 0: an art-less track, an artwork-less
//! entity, Genre Detail's name-hashed stops, and the window between any hero opening and
//! its decode landing. So the failure is invisible on everything that has artwork, which
//! is why it survived two rounds of review on each side.
//!
//! That same reach is why the shared floor now takes a gate: the layer that *is* the
//! backdrop is also the one that eases, and on My Library the globals behind it outlive the
//! tab that filled them.

// A file with its comments dropped, so prose about a fix can neither satisfy a pin nor
// bound a region early. Needed here for a reason particular to this file: every anchor
// below is a gradient literal, and the comment block sitting directly above the floor's
// own binding argues about gradients and stop counts. One line spelling the literal would
// re-anchor both pins ahead of the code.
use crate::test_support::strip_line_comments as code;

const HERO_BLUR: &str = include_str!("../../../melodia-ui/ui/components/hero-blur-backdrop.slint");
const NOW_PLAYING: &str = include_str!("../../../melodia-ui/ui/views/now-playing-view.slint");

/// Both floors ease, on the same token the layers above them take.
///
/// Anchored on the binding rather than searched loosely: `now-playing-view.slint` carries
/// a dozen unrelated `animate` blocks, and what this exists to catch is one line deleted
/// from directly under the gradient.
///
/// Safe to `animate` where a shared component's brush input is not, which is the
/// distinction worth keeping straight — `Brush::interpolate` handles gradient↔gradient
/// stop-for-stop and both sides are two stops at 135deg, and each pair of stops has
/// exactly one writer (`ui::hero_backdrop::write`, `ui::now_playing::track_change`),
/// writing discretely. Nothing can restart either binding mid-flight. See the
/// shared-component entry in `.claude/rules/slint-pitfalls.md` for the case where that
/// isn't true.
#[test]
fn both_backdrop_floors_ease_with_the_layers_above_them() {
    for (file, source) in [
        ("hero-blur-backdrop.slint", HERO_BLUR),
        ("now-playing-view.slint", NOW_PLAYING),
    ] {
        // Anchored on the gradient and then on the end of its own statement, rather than
        // on the line under `background:` — the shared floor swaps its stops for a
        // transparent pair while its host paints no hero, so that binding now spans three
        // lines. What this pin is about is the `animate` sitting directly under it, which
        // both spellings still have to.
        let source = code(source);
        let after = source
            .split_once("@linear-gradient(135deg,")
            .and_then(|(_, rest)| rest.split_once(";\n"))
            .map_or("", |(_, rest)| rest);
        assert_eq!(
            after.lines().next().unwrap_or_default().trim(),
            "animate background { duration: Theme.dur-med; easing: ease-in-out; }",
            "{file}'s gradient floor must ease on `dur-med` directly under its own binding \
             — it is the whole visible backdrop wherever the blur slots sit at 0, so a hard \
             cut there is the view stepping, and it reads as correct on every track or \
             entity that has artwork"
        );
    }
}

/// `HeroBlurBackdrop` is nothing but the three layers, so its duration token can be
/// counted rather than located: two slot opacities, the scrim, and the floor. A layer
/// given a curve of its own is what makes a cover swap and its scrim land a beat apart,
/// which reads as a flicker rather than as a wrong duration.
///
/// Counted over `code` for the reason that helper exists, and a count is the shape prose
/// inflates most easily: one comment line quoting the animate block reads as a fifth layer
/// and fails this for something that is not a regression.
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
/// mosaic bands out of it: they never stop painting one, so they pass nothing and are
/// unchanged. Only `LibraryTabBand` morphs, and only it can hold a `HeroBackdrop` its band
/// stopped painting a tab ago.
///
/// Defaulted the other way every hero would come up on a transparent floor and stay there,
/// which on the one backdrop with no blur over it — Genre Detail's — is the whole banner.
/// **The two arms are checked one at a time, and that is the point rather than pedantry**:
/// an inverted ternary is the same failure as an inverted default, it reads correctly at a
/// glance, and a window holding both arms cannot tell the two spellings apart.
#[test]
fn the_floors_hero_gate_defaults_to_shown() {
    let code = code(HERO_BLUR);

    assert!(
        code.contains("in property <bool> hero-open: true;"),
        "`HeroBlurBackdrop.hero-open` must default to `true` — the two mosaic bands pass no \
         value and always have a hero up, so the default is their whole answer"
    );

    // Split at the ternary's own separators: neither arm is anything but a two-stop
    // `@linear-gradient(135deg, …)`, so neither carries a `?` or a `:` of its own and this
    // lands exactly on the two halves.
    let arms = code
        .split_once("background: root.hero-open")
        .and_then(|(_, rest)| rest.split_once(";\n"))
        .map_or("", |(arms, _)| arms);
    assert!(
        !arms.is_empty(),
        "the shared floor no longer gates its gradient on `hero-open` — both pins below bound \
         against that ternary, and with nothing to split they report an inversion instead"
    );
    let (shown_arm, idle_arm) =
        arms.split_once('?').and_then(|(_, rest)| rest.split_once(':')).unwrap_or(("", ""));

    assert!(
        shown_arm.contains("HeroBackdrop.floor-start")
            && shown_arm.contains("HeroBackdrop.floor-end"),
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
        code.contains("background: HeroBackdrop.scrim;"),
        "the scrim stays ungated — it is solved to a tone that carries almost no chroma, so \
         it has nothing to flash, and draining it would brighten the artwork for the length \
         of every collapse"
    );
}
