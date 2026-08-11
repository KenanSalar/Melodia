//! `Tracks.*` callbacks, split by concern:
//!
//! * [`lifecycle`] — the section gate's shadow and the deferred re-fetch behind it.
//! * [`tracklist`] — the list itself: sort, filter, play, queue, favorite, rating,
//!   selection.
//! * [`columns`] — column-visibility persistence.
//!
//! Same three-file shape as the four sibling library tabs, arrived at late: this slice
//! and Browse were moved into their views by the wiring fold but not split, so both kept
//! one `wire` where the other seven had a directory. What Songs has no equivalent of is a
//! `grid`/`detail` pair — it is one surface, so the middle file is the whole view.

mod columns;
mod lifecycle;
mod tracklist;

use std::sync::Arc;

use crate::AppWindow;
use crate::state::AppState;
use crate::ui::tracks::TracksUi;

/// Wire every `Tracks.*` callback to its `library::*` counterpart and the `tracks_ui`
/// shared state.
///
/// Called by [`super::install`], which is what guarantees the models are in place first;
/// that pairing used to be two statements a boot-file reorder could separate. `wire_all`
/// still has to have run before it.
pub(super) fn wire(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    lifecycle::wire(ui, state, tracks_ui);
    tracklist::wire(ui, state, tracks_ui);
    columns::wire(ui, state);
}
