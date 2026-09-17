//! Which views mount a backdrop stack, and on what.
//!
//! The stack is three layers written twice, once per colour tier, so the question is about the
//! whole Slint tree rather than either component.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// The two stacks, by the name a host mounts them under.
const STACKS: [&str; 2] = ["HeroBlurBackdrop", "AuroraBackdrop"];

/// The files that choose between them, and the only ones allowed to mount either: one wrapper per
/// tier. The six bands mount `HeroBackdropStack` on `HeroBackdrop`, Now Playing and the miniplayer
/// `NpBackdropStack` on `Player.np-*`, and that split is the whole reason the stacks take their
/// colours as inputs.
const SITES: [&str; 2] =
    ["components/hero/hero-backdrop-stack.slint", "components/now-playing/np-backdrop-stack.slint"];

/// Each site mounts each stack exactly once, behind the branch of the setting that paints it.
///
/// **A branch rather than a transparent loser**, safe only because the condition cannot move:
/// `Theme.aurora-backdrop` is an `in` property written once by `boot::ui_setup::apply_backdrop_style`
/// ahead of `install_views` and `app.show()`. What it buys is that the hidden arm stops painting —
/// `Brush::is_transparent()` answers `false` for *every* gradient whatever its stop alphas, so a
/// stack faded to nothing still has its path tessellated and the whole surface filled every frame.
///
/// So what a site can get wrong is the sign: an arm behind the *other* branch reads perfectly and
/// paints the wrong backdrop under the setting the author wasn't on. The local `aurora-shown`
/// property is what keeps the condition single-sourced.
#[test]
fn every_backdrop_site_mounts_only_the_live_arm() {
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
        let gates = ["if !root.aurora-shown:", "if root.aurora-shown:"];
        for (stack, gate) in STACKS.iter().zip(gates) {
            let mounts: Vec<&str> =
                src.lines().filter(|line| line.contains(&format!("{stack} {{"))).collect();
            assert_eq!(
                mounts.len(),
                1,
                "{site} must mount exactly one `{stack}` — a second one paints over the first \
                 under whichever setting reaches it"
            );
            // `starts_with` rather than `contains`: `if root.aurora-shown:` is a substring of the
            // negation, so only anchoring at the line's head tells the two branches apart.
            assert!(
                mounts.iter().all(|line| line.trim_start().starts_with(gate)),
                "{site} must mount `{stack}` behind `{gate}` — ungated it paints over the other \
                 arm for good, and behind the negation it is the wrong backdrop on the setting \
                 the author wasn't using"
            );
        }
    }
}

/// Each wrapper forwards its host's `shown` to whichever stack is mounted. That term is the host's
/// half of the deal: `LibraryTabBand`'s `detail-open` gate and the miniplayer's drain. Drop it and
/// both files still read correctly, while My Library paints a detail's backdrop over its flat
/// state and the miniplayer's backdrop cuts out rather than fading.
#[test]
fn the_wrapper_forwards_its_hosts_gate_to_the_mounted_stack() {
    let tree = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    for site in SITES {
        let src = tree
            .iter()
            .find(|(path, _)| path.ends_with(site))
            .map(|(_, src)| src.as_str())
            .unwrap_or_default();

        for stack in STACKS {
            let mount = src
                .split_once(&format!("{stack} {{"))
                .and_then(|(_, rest)| rest.split_once("\n    }"))
                .map(|(mount, _)| mount)
                .unwrap_or_default();
            assert!(
                mount.contains("shown: root.shown;"),
                "{site}'s `{stack}` must take the wrapper's own `shown`: a host that passes \
                 nothing leaves a missing one correct on the site it was written for and dead on \
                 the one that gates"
            );
        }
    }
}

/// No site gates the aurora on anything but the setting. Each used to AND in a `has-tints` twin,
/// back when an entry with no cover had no washes to lay; `aurora::tints` substitutes the accent as
/// that seed now, so a surviving gate would strand one site on the blur.
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
