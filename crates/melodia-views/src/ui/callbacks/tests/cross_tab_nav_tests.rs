//! Pure pins for the cross-view drill's origin rule.

use super::origin_stamp;
use crate::ui::my_library::NAV_MY_LIBRARY;

/// **An origin is a section, and a drill inside My Library has none.**
///
/// The four destinations are all tabs of that one page, so a drill starting there
/// ends there: the tab bar names the detail's own tab for the whole visit, and a
/// back arrow restoring the tab it came from contradicts the bar beside it.
/// Mouse-4/5 is the control with history semantics, and it still steps back into
/// the detail the drill came from.
///
/// The source-walk half — that every stamp goes through this rather than reading
/// `origin.nav` straight — is `ui::my_library::tests`'.
#[test]
fn origin_stamp_records_a_section_and_never_a_sibling_tab() {
    assert_eq!(
        origin_stamp(NAV_MY_LIBRARY),
        -1,
        "a drill between two tabs of one page records no origin, so its back arrow \
         closes into the grid the tab bar has been naming all along"
    );
    // Search, Browse, Favorites, Recently Played, Settings.
    for nav in [0, 1, 2, 8, 9] {
        assert_eq!(origin_stamp(nav), nav, "section {nav} must record itself");
    }
}
