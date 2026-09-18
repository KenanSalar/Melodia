//! Section enter/leave for the Songs tab, and the one thing it does on the way back in.

use std::sync::Arc;

use slint::{ComponentHandle, Model};

use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::tracks::{self as tracks_ui_mod, TracksUi};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Tracks};

/// Mirror visibility into the synchronous shadow, and on re-enter run the deferred
/// refresh.
///
/// **A tab leave is one of those leaves** — the Songs tab has its own
/// `SectionActiveGate` — so the shadow is seeded from the *mounted tab*, not from the nav
/// index: the gate's `ChangeTracker` baselines inside `AppWindow::new()` and fires only on
/// a later difference, so a view the boot doesn't land on gets no edge at all, and the one
/// it does land on gets its edge a frame late, after boot has already read this shadow.
/// See the `SectionActiveGate` bullet in `.claude/rules/ui-patterns.md`.
///
/// Takes no `&AppState` of its own — it is here for the shape's sake, since every sibling
/// slice's `lifecycle::wire` does, and the deferred path routes back through
/// `request-refresh` rather than fetching itself.
pub(super) fn wire(ui: &AppWindow, _state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let tracks = ui.global::<Tracks>();
    let weak = ui.as_weak();

    tracks_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Songs));

    let tu = tracks_ui.clone();
    tracks.on_section_active_changed(move |active| {
        tu.set_section_active(active);
        // **The leave hands back nothing, and Tracks is the one library view that can say
        // that.** Its four siblings empty their models on the way out, so each owes the
        // `UNFETCHED_COUNT` rewind that stops a count outliving the rows it numbers — and,
        // having rewound, owes the `mark_dirty` that answers it. This model *survives* the
        // leave, so the count is never stale over an empty list and there is nothing to
        // answer. Marking dirty anyway made a tab pick a full `get_tracks` plus a
        // library-sized row build on the event loop, every time the user came back to
        // Songs. Freshness is unaffected:
        // `boot::ui_setup::install_library_changed_refresher` already folds every bump
        // arriving while this tab is unmounted into the same flag.
        if !active {
            // The one exception, and it releases nothing: a selection the user can no longer
            // see still answers `Selection.any-live()`, so Escape over Now Playing would drop
            // it rather than close the page. Its six siblings clear on leave for the same
            // reason. Guarded on there being one, the unstamp walking a model that is
            // library-sized here precisely because it survives.
            if let Some(ui) = weak.upgrade()
                && ui.global::<Tracks>().get_selected_ids().row_count() > 0
            {
                tracks_ui_mod::clear_selection(&ui);
            }
            return;
        }
        if tu.take_dirty() {
            let Some(ui) = weak.upgrade() else { return };
            // Reuse the request-refresh path so the deferred fetch picks up the current
            // sort + filter.
            ui.global::<Tracks>().invoke_request_refresh();
        }
    });
}
