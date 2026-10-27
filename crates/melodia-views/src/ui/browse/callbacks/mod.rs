//! Every `Browse.*` callback, split by concern.

mod folder_actions;
mod lifecycle;
mod navigation;
mod tracklist;
mod view_mode;

use std::sync::Arc;

use crate::ui::browse::BrowseUi;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::AppWindow;

/// Runs after `wire_all`, from [`super::install`], which installs the models first.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    browse_ui: &Arc<BrowseUi>,
) {
    lifecycle::wire(ui, state, browse_ui);
    navigation::wire(ui, state, browse_ui);
    tracklist::wire(ui, state, view_state, browse_ui);
    folder_actions::wire(ui, state);
    view_mode::wire(ui, state, browse_ui);
}
