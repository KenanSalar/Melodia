//! `Browse.*` callbacks, split by concern:
//!
//! * [`lifecycle`] — the section gate, the card tier it releases, and the
//!   `library_changed` re-fetch subscriber.
//! * [`navigation`] — folder open / back / breadcrumb, and the persisted path behind
//!   all three.
//! * [`tracklist`] — the file list: play, queue, favorite, rating, sort, selection.
//! * [`columns`] — column-visibility persistence, under Browse's own `views.json` key.
//! * [`view_mode`] — the list-versus-cards toggle and the private cover tier the grid
//!   draws from.

mod columns;
mod lifecycle;
mod navigation;
mod tracklist;
mod view_mode;

use std::sync::Arc;

use crate::AppWindow;
use crate::ui::browse::BrowseUi;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;

/// Wire every `Browse.*` callback to its `library::*` counterpart and the `browse_ui`
/// shared state, plus a `library_changed` subscriber that re-fetches the current path
/// on watcher events.
///
/// Called by [`super::install`], which is what guarantees the models are in place first — a
/// pairing, rather than two statements a boot-file reorder could separate. `wire_all` still has to
/// have run before it.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    browse_ui: &Arc<BrowseUi>,
) {
    lifecycle::wire(ui, state, browse_ui);
    navigation::wire(ui, state, browse_ui);
    tracklist::wire(ui, state, view_state, browse_ui);
    columns::wire(ui, state);
    view_mode::wire(ui, state, browse_ui);
}
