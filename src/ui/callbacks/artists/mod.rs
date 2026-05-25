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

/// Nav-sidebar index of the Albums tab. Used for cross-tab nav from the
/// Artist Detail Albums sub-section. Kept as a `const` here (next to the
/// only consumers) rather than in `Nav`'s Slint definition because Slint
/// globals can't expose `const`s that Rust reads ergonomically.
pub(super) const NAV_ALBUMS: i32 = 4;

/// Nav-sidebar index of the Artists tab. Mirrors the comment block in
/// `globals.slint::Nav`.
pub(super) const NAV_ARTISTS: i32 = 5;

/// Wire every `Artists.*` / `ArtistDetail.*` callback. Mirrors `wire_albums`,
/// plus an `ArtistDetail.open-album` that flips `Nav.selected-index` to the
/// Albums tab and opens that album's detail.
pub fn wire_artists(
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
