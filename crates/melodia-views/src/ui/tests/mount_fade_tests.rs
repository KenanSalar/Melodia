//! Source pins for the enter fades whose flip lands right after their mount: every page through
//! `ViewTransition`, each notification card, and the miniplayer backdrop's press.
//!
//! An animated binding's first evaluation lands on its value without easing, and Slint builds an
//! `if` branch at input time, well before its first paint, so a 1 ms `Timer` can flip `shown`
//! before anything has read the layers it fades. Whether the fade then arrives at full strength is
//! a race, and neither review nor a single manual run shows it: the miniplayer backdrop snapped on
//! some presses and faded on others. Each faded layer is read once in `init`, which runs before the
//! `Timer` can fire, even where the layers sit in an `if` nested below it.
//!
//! The bare reads look like dead statements, which is what makes them worth pinning: deleting one
//! builds clean and fades correctly on every run that happens to paint first.

use melodia_testkit::strip_line_comments;

const VIEW_TRANSITION: &str =
    include_str!("../../../../melodia-ui/ui/components/view-transition.slint");
const HERO_BLUR: &str =
    include_str!("../../../../melodia-ui/ui/components/hero-blur-backdrop.slint");
const AURORA: &str = include_str!("../../../../melodia-ui/ui/components/aurora-backdrop.slint");
const NOTIFICATION_CARD: &str =
    include_str!("../../../../melodia-ui/ui/components/dialog/notification-card.slint");

/// Every statement in every `init` block of `src`, in source order.
fn init_reads(src: &str) -> Vec<String> {
    strip_line_comments(src)
        .split("init => {")
        .skip(1)
        .filter_map(|block| block.split_once('}').map(|(body, _)| body.to_owned()))
        .flat_map(|body| {
            body.split(';')
                .map(str::trim)
                .filter(|statement| !statement.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What each `animate` block in `src` eases, as spelled, one layer per block.
fn animated_layers(src: &str) -> Vec<String> {
    strip_line_comments(src)
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("animate "))
        .map(|rest| rest.split_once('{').map_or(rest, |(layer, _)| layer).trim().to_owned())
        .collect()
}

/// The reads are exactly `expected`, and `expected` covers every `animate` in the file but the
/// `unbound` ones, so a layer added later fails here until it is read too. An unbound layer eases
/// only what is written to it, leaving `init` no binding to read.
fn assert_every_faded_layer_is_read_at_mount(
    file: &str,
    src: &str,
    expected: &[&str],
    unbound: &[&str],
) {
    let layers = animated_layers(src);
    let stale: Vec<&str> =
        unbound.iter().copied().filter(|layer| !layers.iter().any(|l| l == layer)).collect();
    let bound = layers.iter().filter(|layer| !unbound.contains(&layer.as_str())).count();

    assert_eq!(
        init_reads(src),
        expected,
        "{file}'s `init` no longer reads exactly its faded layers. One left unread is first \
         evaluated after the flip and lands at full strength instead of easing in"
    );
    assert!(stale.is_empty(), "{file} no longer animates {stale:?}, which this pin exempts");
    assert_eq!(
        bound,
        expected.len(),
        "{file} animates a layer its `init` does not read, so that layer snaps where the rest fade"
    );
}

/// Every page mounts through it, flipping `shown` from a 1 ms `Timer`. The 1.17 attempt was
/// reverted in part over this exact miss: pages that appeared instead of sliding in.
#[test]
fn the_view_transition_reads_every_faded_property_at_mount() {
    assert_every_faded_layer_is_read_at_mount(
        "view-transition.slint",
        VIEW_TRANSITION,
        &["root.opacity", "root.x", "root.y"],
        &[],
    );
}

/// The floor, both cover slots and the scrim, all four eased by the miniplayer's press.
#[test]
fn the_blur_stack_reads_every_faded_layer_at_mount() {
    assert_every_faded_layer_is_read_at_mount(
        "hero-blur-backdrop.slint",
        HERO_BLUR,
        &["root.background", "slot-a.opacity", "slot-b.opacity", "scrim-layer.background"],
        &[],
    );
}

/// Two components in one file, each fading one brush: the sweep's, then the base's.
#[test]
fn the_aurora_reads_every_faded_layer_at_mount() {
    assert_every_faded_layer_is_read_at_mount(
        "aurora-backdrop.slint",
        AURORA,
        &["root.background", "root.background"],
        &[],
    );
}

/// Each card's `enter` Timer flips `shown` 1 ms after its row lands, opening the slot as the body
/// fades in. The fade-in once never ran, `shown` being set in `init`.
#[test]
fn the_notification_card_reads_every_faded_layer_at_mount() {
    assert_every_faded_layer_is_read_at_mount(
        "notification-card.slint",
        NOTIFICATION_CARD,
        &["root.fade", "root.slot-h", "root.rise"],
        &["drag-x"],
    );
}
