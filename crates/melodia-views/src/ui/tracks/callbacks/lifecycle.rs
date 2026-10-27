//! Section enter/leave for the Songs tab, and the one thing it does on the way back in.

use std::sync::Arc;

use slint::{ComponentHandle, Model};

use crate::ui::list_selection;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::tracks::TracksUi;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Tracks};

/// Mirrors visibility into the synchronous shadow and runs the deferred refresh on re-enter. The
/// tab has its own gate, so the shadow is seeded off the mounted tab rather than the nav index
/// (`ui-patterns.md`'s `SectionActiveGate` entry). `_state` keeps the siblings' shape; the
/// deferred path goes back through `request-refresh`.
pub(super) fn wire(ui: &AppWindow, _state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let tracks = ui.global::<Tracks>();
    let weak = ui.as_weak();

    tracks_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Songs));

    let tu = tracks_ui.clone();
    tracks.on_section_active_changed(move |active| {
        tu.set_section_active(active);
        // The leave keeps the model, so it rewinds no count and owes no `mark_dirty`; a library
        // change while hidden dirties the flag through the `library_changed` refresher.
        if !active {
            // Only the selection goes back, guarded, the unstamp walking a library-sized model
            // (`ui-patterns.md`'s Escape entry).
            if let Some(ui) = weak.upgrade()
                && ui.global::<Tracks>().get_selected_ids().row_count() > 0
            {
                list_selection::clear_row_selection(&ui.global::<Tracks>());
            }
            return;
        }
        if tu.take_dirty() {
            let Some(ui) = weak.upgrade() else { return };
            // Through `request-refresh`, so the fetch runs under the current sort and filter.
            ui.global::<Tracks>().invoke_request_refresh();
        }
    });
}
