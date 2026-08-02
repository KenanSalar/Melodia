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
/// fires `active-changed` on a real *transition* — with its `ChangeTracker`
/// evaluated inside `AppWindow::new()`, before those handlers exist, so an
/// initial `true` there is notified into an empty slot and lost. Hydrate
/// afterwards and every seed answers for the global's declared default (`3`,
/// Tracks) instead of the restored section, with only that lossy edge left to
/// correct it.
#[test]
fn the_persisted_nav_index_is_hydrated_before_any_view_is_wired() {
    let hydrations = UI_SETUP.matches("set_selected_index(idx)").count();
    assert_eq!(
        hydrations, 1,
        "expected exactly one nav-index hydration site in `install_views`"
    );

    let hydrate = UI_SETUP.find("set_selected_index(idx)");
    let wire_all = UI_SETUP.find("ui::callbacks::wire_all(");
    assert!(wire_all.is_some(), "boot no longer calls `wire_all`");
    assert!(
        hydrate < wire_all,
        "the persisted nav index must be written before `wire_all`: every \
         section's `section_active` shadow seeds off `Nav.selected-index`"
    );
}
