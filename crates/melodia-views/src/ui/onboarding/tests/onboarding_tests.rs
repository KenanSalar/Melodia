const GLOBAL: &str = include_str!("../../../../../melodia-ui/ui/globals/onboarding.slint");
const OVERLAY: &str =
    include_str!("../../../../../melodia-ui/ui/components/onboarding/onboarding-overlay.slint");
const CALLBACKS: &str = include_str!("../callbacks/mod.rs");

/// The panel count Slint declares today. Kept local so a change to `Onboarding.step-count` doesn't
/// silently rewrite what these assert.
const PANELS: usize = 3;

/// `Onboarding.step-count` is the sole definition of how many panels there are — the footer's
/// Next/Done split and Rust's `advance` clamp both read it — but the step dots and the body
/// branches are spelled out, Slint having no way to iterate an integer.
///
/// So three numbers have to agree and nothing in the build notices when they don't: a fourth panel
/// without a bump is unreachable past the third, a bump without a branch shows an empty card with
/// a live Done button, and a dot array left behind just miscounts the progress the user sees.
#[test]
fn the_step_count_matches_the_panels_and_the_dots() {
    let declared = GLOBAL
        .split_once("out property <int> step-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(digits, _)| digits.trim().parse::<usize>().ok());
    assert_eq!(declared, Some(PANELS), "Onboarding.step-count is not the {PANELS} spelled here");

    let branches = OVERLAY.matches("if Onboarding.step ==").count();
    assert_eq!(branches, PANELS, "the overlay mounts {branches} panel branches, not {PANELS}");

    // `for i in [0, 1, 2]:` — the element list, not a `name: [...];` binding, so
    // `melodia_testkit::array_body` doesn't reach it.
    let dots = OVERLAY
        .split_once("for i in [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(body, _)| body.split(',').count());
    assert_eq!(dots, Some(PANELS), "the step-dot array is not {PANELS} long: {dots:?}");
}

/// The deep link out of step 3 writes the tab before the nav index, so Settings mounts on the body
/// it is meant to show rather than on whichever tab it was last left at.
///
/// Both writes go through the callbacks that already own an `IndexPersist`. Reaching for
/// `library::settings::set_settings_tab` here instead would build, persist, and be a seventh
/// writer — `crates/melodia/tests/index_persist.rs` pins that count at six, but it walks
/// `melodia-views` for the *setter*, so it would never see this ordering.
#[test]
fn the_services_deep_link_writes_the_tab_before_the_nav_index() {
    let writes = (
        CALLBACKS.find("invoke_tab_changed(services)"),
        CALLBACKS.find("invoke_persist_selected_index(NAV_SETTINGS)"),
    );
    assert!(
        matches!(writes, (Some(tab), Some(nav)) if tab < nav),
        "the tab must be written before the nav index, and both through the persisting \
         callbacks: {writes:?}"
    );

    assert!(
        !CALLBACKS.contains("set_settings_tab"),
        "the deep link must reuse the tab-changed callback, never the disk setter"
    );
}

/// Every way out of the card means the same thing, and the flag is spent on the way rather than on
/// reaching the last panel: an X, the backdrop, Escape and Skip all reach `dismiss`.
///
/// The guard is what keeps that idempotent — `dismiss` is an `Fn`, and a second Escape inside the
/// fade would otherwise queue a second persist and a second unmount timer.
#[test]
fn a_second_dismiss_inside_the_fade_is_a_no_op() {
    assert!(
        CALLBACKS.contains("if !onboarding.get_open() {"),
        "dismiss must early-out once `open` is already false"
    );
}
