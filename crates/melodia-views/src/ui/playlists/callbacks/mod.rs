//! `Playlists.*` / `PlaylistDetail.*` callbacks. The Add-to-Playlist picker, opener included, is
//! [`files`]'s.

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
use crate::ui::view_ctx::ViewCtx;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::AppWindow;

pub(crate) use crud::PlaylistDeleteUndo;
pub(crate) use detail::PlaylistCutUndo;

/// Wires every `Playlists.*` / `PlaylistDetail.*` callback but [`wire_files`]', on
/// `albums::callbacks::wire`'s terms.
pub(super) fn wire(cx: ViewCtx<'_>, playlists_ui: &Arc<PlaylistsUi>) {
    let ViewCtx { app: ui, state, .. } = cx;
    grid::wire(ui, state, playlists_ui);
    detail::wire(cx, playlists_ui);
    row_cover::wire(ui, playlists_ui);
    crud::wire(cx, playlists_ui);
    artwork::wire(ui, state, playlists_ui);
    smart::wire(ui, state, playlists_ui);
    lifecycle::wire(ui, state, playlists_ui);
}

/// Refresh the grid after an edit, and the open detail too where it is one of `edited`.
async fn refresh_after_edit(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    edited: &[i64],
    label: &str,
) {
    playlists_ui_mod::refresh_after_write(state, playlists_ui, &weak);
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

/// See `ui::playlists::wire_files`.
pub fn wire_files(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    files::wire(ui, state, playlists_ui, notifications);
}
