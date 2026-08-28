//! Which Radio sub-view is mounted, and how that answer is seeded.
//!
//! The indices live in `globals/radio.slint`'s `tab-*` constants and no Rust file restates them.

use slint::ComponentHandle;

use crate::{AppWindow, Nav, Radio};

/// Which Radio sub-view is mounted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RadioTab {
    Browse,
    Favorites,
    Recent,
}

impl RadioTab {
    /// The variants, for the walks over per-tab state — the three station-page seats, and the
    /// artwork each holds. **A variant added to the enum and left out of here still compiles**,
    /// and every walk then skips that tab quietly rather than failing, so the length is pinned
    /// against `Radio.tab-count` in `super::tests` rather than trusted to the array.
    pub const ALL: [Self; 3] = [Self::Browse, Self::Favorites, Self::Recent];
}

/// Resolve a `Radio.tab-idx` value against the global's own `tab-*` constants. UI thread only —
/// that's where the global is reachable.
///
/// Ends in a default arm, so a tab added to `radio.slint` without one here resolves to `Browse`
/// and `ui::view_tag` logs that.
pub fn tab_from_index(g: &Radio<'_>, idx: i32) -> RadioTab {
    if idx == g.get_tab_favorites() {
        RadioTab::Favorites
    } else if idx == g.get_tab_recent() {
        RadioTab::Recent
    } else {
        RadioTab::Browse
    }
}

/// Whether the page is the section on screen — the wire-time seed for [`super::RadioUi`]'s
/// `section` shadow. Deliberately blind to the tab: the page mounts one gate for all three.
pub fn section_is_up(ui: &AppWindow) -> bool {
    ui.global::<Nav>().get_selected_index() == super::NAV_RADIO
}

/// Seed the active tab from `views.json`, clamped against the Slint-declared `tab-count`
/// (see [`crate::ui::tab_bar::clamp_tab`]).
///
/// **Called from `install_views` beside the nav-index hydration**, which is the one place
/// `views.json` is in hand, and ahead of the slice's own `install` — that is where the mounted
/// tab's model gets filled, so the tab has to be the restored one by then rather than the
/// global's declared `0`.
///
/// The five-gates ordering argument is My Library's, not this page's: one gate for every tab
/// means the section shadow reads the nav index and never the tab.
///
/// Stateless, like My Library's: there is no handle to seed at that point.
pub fn seed_tab(ui: &AppWindow, persisted_tab: i32) {
    let g = ui.global::<Radio>();
    g.set_tab_idx(crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count()));
}
