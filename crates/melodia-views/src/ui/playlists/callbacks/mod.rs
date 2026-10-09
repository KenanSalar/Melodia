//! `Playlists.*` / `PlaylistDetail.*` callbacks, split by concern:
//!
//! * [`grid`] — the playlist-card grid (cover lookup, filter, drill-in,
//!   the Rename-overlay name / description lookups).
//! * [`detail`] — the open-playlist detail view: close-detail, drag-reorder,
//!   edit-artwork open and track removal, plus the set every detail shares.
//! * [`crud`] — the create / rename / delete commits.
//! * [`artwork`] — the Edit Artwork picker: the grid card's opener and the body [`detail`]'s
//!   shares, the mosaic-candidate toggle and the apply / clear commits.
//! * [`row_cover`] — the row-tier cover lookup every picker dialog draws through.
//! * [`smart`] — the smart-playlist rule editor.
//! * [`lifecycle`] — section enter/leave cache management + the
//!   `library_changed` re-fetch subscriber.
//!
//! The Add-to-Playlist picker, opener included, is [`files`]'s.

mod artwork;
mod crud;
mod detail;
mod files;
mod grid;
mod lifecycle;
mod row_cover;
mod smart;

use std::rc::Rc;
use std::sync::Arc;

use slint::Weak;

use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::shell::notifications::NotificationsUi;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::AppWindow;

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
    row_cover::wire(ui, playlists_ui);
    crud::wire(ui, state, playlists_ui);
    artwork::wire(ui, state, playlists_ui);
    smart::wire(ui, state, playlists_ui);
    lifecycle::wire(ui, state, playlists_ui);
}

/// Re-fetch the grid after an edit, and the open detail too where it is one of `edited`.
async fn refresh_after_edit(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    edited: &[i64],
    label: &str,
) {
    if let Err(e) = playlists_ui_mod::fetch_grid(state, playlists_ui, weak.clone()).await {
        log::warn!("{label} refetch grid: {}", describe(&e));
    }
    let open_id = playlists_ui.detail_playlist_id();
    if edited.contains(&open_id)
        && let Err(e) = playlists_ui_mod::refresh_detail(state, playlists_ui, weak, open_id).await
    {
        log::warn!("{label} refresh detail: {}", describe(&e));
    }
}

/// A description as stored: blank means none, so the database holds NULL rather than an empty
/// string.
fn optional_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Wire the M3U8 import / export callbacks and the Add-to-Playlist picker. Split out of [`wire`]
/// because each needs the `Rc<NotificationsUi>` for completion toasts, which is created after the
/// per-view wiring runs. Boot's `install_ui` calls it once, through `ui::playlists::wire_files`.
pub fn wire_files(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    files::wire(ui, state, playlists_ui, notifications);
}
