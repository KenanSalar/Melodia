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
    for (file, source) in
        [("hero-blur-backdrop.slint", HERO_BLUR), ("now-playing-view.slint", NOW_PLAYING)]
    {
        let after = source
            .split_once("background: @linear-gradient(135deg,")
            .and_then(|(_, rest)| rest.split_once('\n'))
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
#[test]
fn the_shared_backdrop_rides_one_duration() {
    assert_eq!(
        HERO_BLUR.matches("duration: Theme.dur-med").count(),
        4,
        "`HeroBlurBackdrop` is four animations on one token — both blur slots, the scrim \
         and the gradient floor"
    );
}
