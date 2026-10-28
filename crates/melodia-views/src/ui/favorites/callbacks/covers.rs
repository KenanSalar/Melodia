//! The two grid tabs' cover lookups; Songs rows resolve through `RowCovers` like every list's.

use slint::ComponentHandle;

use melodia_ui::{AppWindow, Favorites};

/// `generation` is read for its effect on the binding, never its value; the shared tier's one
/// notifier is `boot::ui_setup::views::install_grid_covers`.
pub(super) fn wire(ui: &AppWindow) {
    let g = ui.global::<Favorites>();
    g.on_request_most_played_cover(|path, _generation| {
        crate::ui::grid_prewarm::grid_cover(path.as_str())
    });
    g.on_request_artist_cover(|path, _generation| {
        crate::ui::grid_prewarm::grid_cover(path.as_str())
    });
}
