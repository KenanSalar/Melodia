//! `Tracks.*` callbacks: [`lifecycle`] (the section gate's shadow and the deferred
//! re-fetch behind it), [`tracklist`] (the list itself — sort, filter, play, queue,
//! favorite, rating, selection) and [`columns`] (column-visibility persistence).
//!
//! The same three-file shape as the four sibling library tabs. What Songs has no
//! equivalent of is a `grid`/`detail` pair — it is one surface, so the middle file is
//! the whole view.

mod columns;
mod lifecycle;
mod tracklist;

use std::sync::Arc;

use crate::AppWindow;
use crate::services::view_state::ViewStateData;
use crate::state::AppState;
use crate::ui::tracks::TracksUi;

/// Wire every `Tracks.*` callback to its `library::*` counterpart and the `tracks_ui`
/// shared state.
///
/// Called by [`super::install`], which is what guarantees the models are in place first — a
/// pairing, rather than two statements a boot-file reorder could separate. `wire_all` still has to
/// have run before it.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    tracks_ui: &Arc<TracksUi>,
) {
    lifecycle::wire(ui, state, tracks_ui);
    tracklist::wire(ui, state, view_state, tracks_ui);
    columns::wire(ui, state);
}
