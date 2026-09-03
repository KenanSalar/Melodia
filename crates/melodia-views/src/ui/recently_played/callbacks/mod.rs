//! `RecentlyPlayed.*` callbacks, split by concern:
//!
//! * [`covers`] — the Most Played grid's lazy cover-lookup callback.
//! * [`subviews`] — the Most Played card actions, the tab switch, and the
//!   grid's column-count push.
//! * [`tracklist`] — the Songs tab: row actions, filter, column visibility,
//!   modifier-aware selection, and its Shuffle pill.
//! * [`lifecycle`] — section enter/leave cache management + the joined
//!   `library_changed` + `stats_changed` re-fetch subscriber.

mod covers;
mod lifecycle;
mod subviews;
mod tracklist;

use std::sync::Arc;

use crate::AppWindow;
use crate::ui::recently_played::RecentlyPlayedUi;
use melodia_app::state::AppState;

/// The settings view-id under which this view's column state persists. Unlike
/// its sortable siblings there is no `view_sort` entry — the Songs list is
/// mounted non-sortable, so recency is the only order it has, and the Most
/// Played tab's title names its own.
const VIEW_ID: &str = crate::ui::track_list_view::view_id::RECENTLY_PLAYED;

/// Wire every `RecentlyPlayed.*` callback.
///
/// Called by [`super::install`], which is what guarantees the models are in place first — a
/// pairing, rather than two statements a boot-file reorder could separate. `wire_all` still has to
/// have run before it.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    covers::wire(ui, rp_ui);
    subviews::wire(ui, state, rp_ui);
    tracklist::wire(ui, state, rp_ui);
    lifecycle::wire(ui, state, rp_ui);
}
