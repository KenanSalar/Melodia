//! Source-order pins for the boot sequence.
//!
//! Nothing at runtime can catch what these hold: the app builds, boots and
//! paints either way, and only the *section it boots into* behaves wrongly.

const UI_SETUP: &str = include_str!("../ui_setup.rs");

/// The persisted nav index has to reach `Nav.selected-index` before any
/// `wire_*` runs.
///
/// Each of the nine section handles seeds its synchronous `section_active`
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
