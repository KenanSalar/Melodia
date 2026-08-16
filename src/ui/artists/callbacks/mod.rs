//! `Artists.*` / `ArtistDetail.*` callbacks, split by concern:
//!
//! * [`grid`] — the artist-card grid (cover lookup, filter / sort, drill-in).
//! * [`detail`] — the open-artist detail view (play, queue, favorite, sort,
//!   filter, Albums sub-section collapse).
//! * [`cross_tab`] — the Artist Detail → Albums tab hand-off.
//! * [`lifecycle`] — section enter/leave cache management + the
//!   `library_changed` re-fetch subscriber.

mod cross_tab;
mod detail;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::AppWindow;
use crate::state::AppState;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;

/// Wire every `Artists.*` / `ArtistDetail.*` callback. Mirrors the Albums
/// slice's, plus an `ArtistDetail.open-album` that moves to the Albums tab and
/// opens that album's detail.
///
/// Called by [`super::install`], which is what guarantees the models are in place first — a
/// pairing, rather than two statements a boot-file reorder could separate. `wire_all` still has to
/// have run before it.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    albums_ui: &Arc<AlbumsUi>,
) {
    grid::wire(ui, state, artists_ui);
    detail::wire(ui, state, artists_ui, albums_ui);
    cross_tab::wire(ui, state, albums_ui);
    lifecycle::wire(ui, state, artists_ui, albums_ui);
}
