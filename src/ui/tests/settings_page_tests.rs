use super::clamp_tab;

/// The tab count Slint declares today. Kept local so a change to
/// `SettingsPage.tab-count` doesn't silently rewrite what these assert.
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
/// while the global declares five tabs, but the arithmetic shouldn't be the
/// thing that decides that.
#[test]
fn clamp_tab_survives_a_zero_tab_count() {
    assert_eq!(clamp_tab(0, 0), 0);
    assert_eq!(clamp_tab(7, 0), 0);
}
