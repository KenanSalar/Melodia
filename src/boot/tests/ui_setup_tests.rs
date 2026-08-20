//! Source-order pins for the boot sequence.
//!
//! Nothing at runtime can catch what these hold: the app builds, boots and
//! paints either way, and only the *section it boots into* behaves wrongly.

const UI_SETUP: &str = include_str!("../ui_setup.rs");
const MAIN: &str = include_str!("../../main.rs");

/// The persisted nav index has to reach `Nav.selected-index` before any
/// `wire_*` runs.
///
/// Each of the ten section handles seeds its synchronous `section_active`
/// shadow by reading that property at wire time, and `SectionActiveGate` only
/// fires `active-changed` on a real *transition*. Its `ChangeTracker` is
/// evaluated inside `AppWindow::new()` and adopts that first value **silently**
/// — `ChangeTracker::init` assigns it and never calls the notify half
/// (`init_delayed` is the variant that would, and the generated tree uses it
/// nowhere) — so the boot reading never reaches Rust and becomes the baseline
/// every later evaluation is compared against.
///
/// Hydrate afterwards and every seed answers for the global's declared default
/// (`3`, Tracks), leaving `TracksUi` marked active for a section that isn't on
/// screen: the gate's baseline is already `false`, so it has no `false` edge
/// left to deliver.
#[test]
fn the_persisted_nav_index_is_hydrated_before_any_view_is_wired() {
    // Both anchors are pinned by count first, so the ordering assert below can
    // only ever fail for the reason its message gives — an anchor that went
    // missing reports itself here instead.
    let hydrations = UI_SETUP.matches("set_selected_index(").count();
    assert_eq!(hydrations, 1, "expected exactly one nav-index hydration site in `install_views`");
    let wire_alls = UI_SETUP.matches("ui::callbacks::wire_all(").count();
    assert_eq!(wire_alls, 1, "boot no longer calls `wire_all`");

    // Neither `find` can be `None` past those two asserts; the `unwrap_or`s are
    // there because the crate denies `unwrap`.
    let hydrate = UI_SETUP.find("set_selected_index(").unwrap_or(usize::MAX);
    let wire_all = UI_SETUP.find("ui::callbacks::wire_all(").unwrap_or(0);
    assert!(
        hydrate < wire_all,
        "the persisted nav index must be written before `wire_all`: every \
         section's `section_active` shadow seeds off `Nav.selected-index`"
    );
}

/// The backdrop setting has to reach `Theme.aurora-backdrop` before `install_views`.
///
/// Unlike its two siblings the anchors are in `main.rs`, the flag being raised there rather
/// than inside `install_views` — the call it has to precede. `install_views` builds the three
/// `DetailArtwork` tiers, whose `BlurSpec` is decided from this flag, then seeds all four
/// detail views, whose fetches reach `ui::backdrop::kind`. Raise it later and the tiers blur
/// every cover the aurora will never paint.
///
/// The mutation this catches is the obvious one: moving the read into
/// `hydrate_ui_from_settings`, where every other `settings.json`-to-Slint apply lives.
#[test]
fn the_backdrop_setting_is_hydrated_before_the_views_are_installed() {
    let hydrations = MAIN.matches("apply_backdrop_style(").count();
    assert_eq!(hydrations, 1, "expected exactly one backdrop-style hydration site in `main`");
    let installs = MAIN.matches("install_views(").count();
    assert_eq!(installs, 1, "boot no longer calls `install_views`");

    let hydrate = MAIN.find("apply_backdrop_style(").unwrap_or(usize::MAX);
    let install_views = MAIN.find("install_views(").unwrap_or(0);
    assert!(
        hydrate < install_views,
        "the persisted backdrop style must be raised before `install_views`: the artwork \
         tiers it builds decide there whether to hold a blur spec at all"
    );
}

/// The My Library tab has to reach `MyLibrary.tab-idx` before any `wire_*` runs, for
/// the same reason and with a sharper failure.
///
/// Five sub-views share nav index 3, so each seeds its `section_active` shadow from
/// `Nav.selected-index == 3 && MyLibrary.tab-idx == <its tab>`. Seed the tab afterwards
/// and every one of them reads the global's declared `0`: Songs comes up active and the
/// tab actually being restored comes up inactive, with no gate edge left to correct
/// either (see the test above for why). The visible cost is a full Tracks query on every
/// launch that resumes anywhere but Songs, and the restored tab's own fetch landing late
/// behind it.
///
/// The mutation this catches is moving the call down beside `favorites::seed_tab` and
/// `recently_played::seed_tab`, which is where a tab seed looks like it belongs — those
/// two wait because they also seed a handle's shadow, and this one has no handle.
#[test]
fn the_persisted_my_library_tab_is_seeded_before_any_view_is_wired() {
    let seeds = UI_SETUP.matches("ui::my_library::seed_tab(").count();
    assert_eq!(seeds, 1, "expected exactly one My Library tab seed in `install_views`");
    let wire_alls = UI_SETUP.matches("ui::callbacks::wire_all(").count();
    assert_eq!(wire_alls, 1, "boot no longer calls `wire_all`");

    let seed = UI_SETUP.find("ui::my_library::seed_tab(").unwrap_or(usize::MAX);
    let wire_all = UI_SETUP.find("ui::callbacks::wire_all(").unwrap_or(0);
    assert!(
        seed < wire_all,
        "the persisted My Library tab must be written before `wire_all`: all five of \
         that page's sections seed `section_active` from it"
    );
}

/// The retune has to be **registered as well as scheduled**, and the two are one closure so
/// nothing warns when only half of it survives.
///
/// The post-show call answers the tiers' first read, at which point the window is on a monitor
/// and its scale factor is real. Every later answer comes off `WindowChrome.display-changed`,
/// fired by the winit `Resized` filter — without that registration a cap sized for the launch
/// window stands for the session, and a maximize afterwards mounts more cards than the tier
/// holds. The failure is silent: the overflow paints placeholders and everything else works.
#[test]
fn the_cover_retune_is_wired_to_resizes_as_well_as_to_the_first_show() {
    assert_eq!(
        UI_SETUP.matches("on_display_changed(").count(),
        1,
        "`install_views` must register the cover retune against `WindowChrome.display-changed`, \
         or the caps stay sized for whatever the window was at launch"
    );
    assert_eq!(
        UI_SETUP.matches("invoke_from_event_loop(retune)").count(),
        1,
        "the same closure owes the deferred first call: read inline it finds no window, and \
         every tier silently takes its construction fallback"
    );
}
