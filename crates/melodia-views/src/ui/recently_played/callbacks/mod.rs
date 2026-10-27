//! Every `RecentlyPlayed.*` callback, split by concern.

mod covers;
mod lifecycle;
mod subviews;
mod tracklist;

use std::sync::Arc;

use crate::ui::recently_played::RecentlyPlayedUi;
use melodia_app::state::AppState;
use melodia_ui::AppWindow;

/// Runs after `wire_all`, from [`super::install`], which installs the models first.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    covers::wire(ui);
    subviews::wire(ui, state, rp_ui);
    tracklist::wire(ui, state, rp_ui);
    lifecycle::wire(ui, state, rp_ui);
}
