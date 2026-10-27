//! Which Radio sub-view is mounted, and how that answer is seeded.
//!
//! The indices live in `globals/radio.slint`'s `tab-*` constants and no Rust file restates them.

use slint::ComponentHandle;

use melodia_ui::{AppWindow, Nav, Radio};

/// Which Radio sub-view is mounted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RadioTab {
    Browse,
    Favorites,
    Recent,
}

impl RadioTab {
    /// Every variant, for the walks over per-tab state. A variant left out still compiles and is
    /// skipped quietly, so `super::tests` pins the length against `Radio.tab-count`.
    pub const ALL: [Self; 3] = [Self::Browse, Self::Favorites, Self::Recent];
}

/// Resolves a `Radio.tab-idx` value against the global's own `tab-*` constants, a tab with no arm
/// here falling to `Browse`, which `ui::view_tag` logs.
pub fn tab_from_index(g: &Radio<'_>, idx: i32) -> RadioTab {
    if idx == g.get_tab_favorites() {
        RadioTab::Favorites
    } else if idx == g.get_tab_recent() {
        RadioTab::Recent
    } else {
        RadioTab::Browse
    }
}

/// The tab currently mounted: [`tab_from_index`] asked about the global's own index.
pub fn mounted_tab(g: &Radio<'_>) -> RadioTab {
    tab_from_index(g, g.get_tab_idx())
}

/// Whether the page is the section on screen — the wire-time seed for [`super::RadioUi`]'s
/// `section` shadow. Deliberately blind to the tab: the page mounts one gate for all three.
pub fn section_is_up(ui: &AppWindow) -> bool {
    ui.global::<Nav>().get_selected_index() == super::NAV_RADIO
}

/// Seeds the active tab from `views.json`, clamped against `Radio.tab-count` through
/// [`crate::ui::tab_bar::clamp_tab`]. Called from `install_views` ahead of the slice's own
/// `install`, which fills the mounted tab's model, so the tab is the restored one by then.
pub fn seed_tab(ui: &AppWindow, persisted_tab: i32) {
    let g = ui.global::<Radio>();
    g.set_tab_idx(crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count()));
}
