//! `Playlists.*` / `PlaylistDetail.*` callbacks, split by concern:
//!
//! * [`grid`] — the playlist-card grid (cover lookup, filter, drill-in,
//!   the Rename-overlay name / description lookups).
//! * [`detail`] — the open-playlist detail view (play, queue, favorite,
//!   sort, drag-reorder, edit-artwork open, track removal).
//! * [`dialog`] — the create / rename / delete / mosaic-artwork CRUD
//!   commits, the add-to-playlist picker, and mosaic-candidate toggling.
//! * [`lifecycle`] — section enter/leave cache management + the
//!   `library_changed` re-fetch subscriber.

mod detail;
mod dialog;
mod files;
mod grid;
mod lifecycle;
mod smart;

use std::rc::Rc;
use std::sync::Arc;

use crate::AppWindow;
use crate::ui::playlists::PlaylistsUi;
use crate::ui::shell::notifications::NotificationsUi;
use melodia_app::state::AppState;

/// Wire every `Playlists.*` / `PlaylistDetail.*` callback to its
/// `library::*` counterpart and the `playlists_ui` shared state, plus a
/// `library_changed` subscriber that re-fetches the grid (and refreshes
/// an open detail) on watcher / scan / CRUD events.
///
/// Called by [`super::install`], which is what guarantees the models are in place first — a
/// pairing, rather than two statements a boot-file reorder could separate. `wire_all` still has to
/// have run before it.
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    grid::wire(ui, state, playlists_ui);
    detail::wire(ui, state, playlists_ui);
    dialog::wire(ui, state, playlists_ui);
    smart::wire(ui, state, playlists_ui);
    lifecycle::wire(ui, state, playlists_ui);
}

/// Wire the playlist import/export (M3U8) callbacks. Split out of [`wire`]
/// because it needs the `Rc<NotificationsUi>` for completion toasts, which is
/// created after the per-view wiring runs. Call once from `main.rs` after the
/// notifications stack exists — re-exported as `ui::playlists::wire_files`.
pub fn wire_files(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    files::wire(ui, state, playlists_ui, notifications);
}
