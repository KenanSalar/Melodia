//! The Most Played grid's cover lookup; Songs rows resolve through `RowCovers` like every list's.

use slint::ComponentHandle;

use melodia_ui::{AppWindow, RecentlyPlayed};

/// `generation` is read for its effect on the binding, never its value: it re-runs the lookup
/// once a decode lands behind a mounted grid.
pub(super) fn wire(ui: &AppWindow) {
    ui.global::<RecentlyPlayed>().on_request_most_played_cover(|path, _generation| {
        crate::ui::grid_prewarm::grid_cover(path.as_str())
    });
}
