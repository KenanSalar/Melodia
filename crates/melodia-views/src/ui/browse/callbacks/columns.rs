//! Column-visibility persistence for the Browse file list.

use slint::ComponentHandle;

use crate::ui::callbacks::macros::spawn_blocking_logged;
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Browse};

/// toggle-column: the popup already flipped the matching `show-*` flag for instant visual
/// feedback. Persist the new visible-column list to `views.json`'s
/// `view_columns["browse"]` — a separate key from the Tracks view, so Browse keeps its own
/// column layout.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Browse>().on_toggle_column(move |_id| {
        let Some(ui) = weak.upgrade() else { return };
        let columns = ui.global::<Browse>().snapshot_visible();
        let s = s.clone();
        spawn_blocking_logged!(
            s,
            "browse::toggle_column",
            library::settings::update_view_columns(&s, view_id::BROWSE.to_owned(), columns)
        );
    });
}
