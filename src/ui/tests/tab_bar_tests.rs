use super::clamp_tab;

/// A representative tab count. The real ones live in each host's Slint global
/// and are pinned there; this is just a fixture for the arithmetic.
const TABS: i32 = 5;

#[test]
fn clamp_tab_passes_through_valid_indices() {
    for tab in 0..TABS {
        assert_eq!(clamp_tab(tab, TABS), tab);
    }
}

#[test]
fn clamp_tab_pulls_out_of_range_back_in() {
    // A `views.json` from a build with more tabs, and a corrupt negative.
    assert_eq!(clamp_tab(99, TABS), TABS - 1);
    assert_eq!(clamp_tab(TABS, TABS), TABS - 1);
    assert_eq!(clamp_tab(-1, TABS), 0);
}

/// `clamp(0, -1)` panics, so the upper bound is floored at 0. Not reachable
/// while both globals declare tabs, but the arithmetic shouldn't be the thing
/// that decides that.
#[test]
fn clamp_tab_survives_a_zero_tab_count() {
    assert_eq!(clamp_tab(0, 0), 0);
    assert_eq!(clamp_tab(7, 0), 0);
}

const TAB_BAR: &str = include_str!("../../../melodia-ui/ui/components/tab-bar.slint");

/// The compact morph has to be *written*, not left to the binding that seeds
/// it. Slint restarts an animated binding whenever a dependency is marked
/// dirty — `AnimatedBindingCallable::mark_dirty` resets the start time and
/// re-bases the from-value, with no check that the value changed — and
/// `compact` reads `avail-width`, which a resize drag rewrites on every pointer
/// motion. Bound, the 350 ms curve was torn down every few milliseconds and the
/// bar crawled toward its target at whatever rate the drag delivered events.
/// The write in `changed compact` swaps that binding for an animation of its
/// own and is the entire fix — and it is invisible in the source, since it sits
/// one line under a binding computing the same thing. Delete it and the file
/// still builds, still looks right, and stutters again.
#[test]
fn the_compact_morph_is_written_not_bound() {
    assert!(
        TAB_BAR.contains("animate compact-t"),
        "tab-bar.slint must still ease `compact-t` — this test guards how it's driven"
    );

    // `changed is-hovered` is the only sibling handler, so this anchor is
    // unambiguous; a miss leaves `handler` empty and fails below rather than
    // passing vacuously.
    let handler = TAB_BAR
        .split_once("changed compact =>")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
        .unwrap_or_default();

    assert!(
        handler.contains("root.compact-t ="),
        "`changed compact` must write `compact-t`. Left to its binding, the morph restarts on \
         every resize event of a drag instead of playing its own curve"
    );
}

/// Splitting the bar's `width` into `min`/`preferred`/`max` is what keeps the
/// morph off the window's own minimum, and it buys that by letting the layout
/// draw the bar narrower than it asked for. On the shrink leg `compact` flips
/// the instant the threshold is crossed while `tab-w` takes 350 ms to follow, so
/// `preferred-width` is still a row of natural cells against a header that can
/// no longer seat them — and the cells bind their widths, so they can't
/// compress. Without the clip they paint under the search input. Rectangular
/// and borderless is the point: that lowers to a scissor rather than the
/// offscreen layer a rounded clip over text would cost.
#[test]
fn the_bar_clips_what_the_width_split_lets_it_overdraw() {
    // Anchored past `TabBarCell`, which is declared above the bar in the same
    // file and clips its own label slot — an unanchored search would pass on
    // that one and never notice the root's going missing.
    let bar = TAB_BAR.split_once("export component TabBar").map(|(_, body)| body).unwrap_or_default();

    assert!(
        bar.contains("clip: true"),
        "tab-bar.slint's root must clip — the min/preferred/max split lets the layout draw it \
         narrower than its cells, and their bound widths spill under the search bar"
    );
}

/// Every brush the bar paints with has to be reachable from the call site.
/// The bar is mounted on a hero blur as well as on a page background, and
/// `ui-patterns.md` is explicit that anything on a hero reads `HeroBackdrop`
/// rather than a `Theme.*` token — a hardcoded `Theme.text` label or
/// `Theme.surface1` divider looks correct in Settings and washes out on the
/// banner. Defaults keep Settings on the tokens it always used.
#[test]
fn every_painted_brush_is_an_input() {
    for prop in ["label-color", "active-color", "hover-fill", "divider-color"] {
        assert!(
            TAB_BAR.contains(&format!("in property <brush> {prop}:")),
            "tab-bar.slint must expose `{prop}` as a defaulted `in property <brush>` — a host on \
             a hero backdrop can't reach a hardcoded Theme token"
        );
    }

    // The cells and the underline must read those inputs, not the tokens
    // directly. `Theme.*` still appears in the file for geometry, durations and
    // the defaults themselves, so anchor on the two paint sites that regressed.
    let bar = TAB_BAR.split_once("export component TabBar").map(|(_, body)| body).unwrap_or_default();
    assert!(
        !bar.contains("background: Theme.surface1"),
        "the divider must paint `divider-color`, not `Theme.surface1` directly"
    );
    assert!(
        !bar.contains("background: Theme.accent"),
        "the underline must paint `active-color`, not `Theme.accent` directly"
    );
}
