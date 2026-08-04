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
    assert_eq!(
        hydrations, 1,
        "expected exactly one nav-index hydration site in `install_views`"
    );
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
