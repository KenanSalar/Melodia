//! The row-tier cover lookup the three picker dialogs draw through: the mosaic
//! picker's candidate tiles and preview slots, Add to Playlist and Export.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::playlists::PlaylistsUi;
use melodia_ui::{AppWindow, Playlists};

/// Shares the row-tier `cover_thumbs` LRU with Tracks / Browse / detail track-lists.
pub(super) fn wire(ui: &AppWindow, playlists_ui: &Arc<PlaylistsUi>) {
    let pu = playlists_ui.clone();
    ui.global::<Playlists>().on_request_row_cover(move |path| pu.row_cover(path.as_str()));
}
