//! The launch's reopen of each detail's last entity, kept for the tab's first enter where the
//! launch shows another one, for the reason [`SectionState::defer_restore`] gives.
//!
//! [`SectionState::defer_restore`]: crate::ui::section_state::SectionState::defer_restore

use std::sync::Arc;

use slint::Weak;

use super::TrackDetail;
use crate::ui::my_library::tab_is_mounted;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::AppWindow;

/// A view's own reopen of the entity `id`, which raises and lowers its `restoring` flag.
pub type Restore<V> = fn(&AppWindow, &AppState, &Arc<V>, i64);

/// Reopens the entity `view` showed at the last shutdown, or keeps it for the tab's first enter.
pub fn seed_or_defer<V: TrackDetail>(
    ui: &AppWindow,
    state: &AppState,
    view: &Arc<V>,
    restore: Restore<V>,
) {
    let Some(id) = library::settings::get_view_state(&state.paths)
        .ok()
        .and_then(|s| s.last_detail_ids.get(V::VIEW_ID).copied())
    else {
        return;
    };
    if !tab_is_mounted(ui, V::TAB) {
        view.section().defer_restore(id);
        return;
    }
    restore(ui, state, view, id);
}

/// Reopens what [`seed_or_defer`] kept, on the tab's first enter.
pub fn resume_deferred<V: TrackDetail>(
    weak: &Weak<AppWindow>,
    state: &AppState,
    view: &Arc<V>,
    restore: Restore<V>,
) {
    if let Some(id) = view.section().take_deferred_restore(|| view.detail_open())
        && let Some(ui) = weak.upgrade()
    {
        restore(&ui, state, view, id);
    }
}
