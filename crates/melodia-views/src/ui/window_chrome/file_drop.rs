//! Where a file dragged in from the file manager lands.
//!
//! No OS drag reaches a Slint `DropArea`, so the winit filter routes it: the vendored winit fork
//! reports the pointer on every move, and [`route`] hit-tests it against where the two targets are
//! drawn, which they publish on `FileDrag`. The banner a move raises and the import a drop starts
//! both come from [`route`], so a banner always names where the files will go.

use std::path::PathBuf;

use slint::{ComponentHandle, LogicalPosition};

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_core::utils::toast::{self, ToastKind};
use melodia_ui::{AppWindow, FileDrag, FileDropTarget, Queue, WindowRect};

/// Where letting go of a file drag lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DropRoute {
    Queue,
    Playlist(i64),
    Nowhere,
}

impl DropRoute {
    /// The banner that says so.
    pub(super) fn target(self) -> FileDropTarget {
        match self {
            DropRoute::Queue => FileDropTarget::Queue,
            DropRoute::Playlist(_) => FileDropTarget::Playlist,
            DropRoute::Nowhere => FileDropTarget::None,
        }
    }
}

/// The targets that can take a drop right now, and where each is drawn.
pub(super) struct DropTargets {
    queue: Option<WindowRect>,
    /// The content panel, and the playlist a drop there lands in.
    playlist: Option<(WindowRect, i64)>,
}

/// The queue panel as the window last reported it.
struct QueuePanel {
    shown: bool,
    floating: bool,
    rect: WindowRect,
}

impl DropTargets {
    pub(super) fn read(ui: &AppWindow) -> Self {
        let queue = ui.global::<Queue>();
        let drag = ui.global::<FileDrag>();
        let panel = QueuePanel {
            shown: queue.get_shown(),
            floating: queue.get_floating(),
            rect: drag.get_queue_rect(),
        };
        let playlist =
            crate::ui::playlists::detail_on_screen(ui).map(|id| (drag.get_content_rect(), id));
        Self::new(panel, playlist)
    }

    /// What the queue `panel` leaves taking drops, `playlist` being the detail on screen.
    fn new(panel: QueuePanel, playlist: Option<(WindowRect, i64)>) -> Self {
        // Under the overlay the page is dimmed and takes no click, so it takes no drop either.
        let playlist = if panel.floating { None } else { playlist };
        Self { queue: panel.shown.then_some(panel.rect), playlist }
    }
}

/// Where a drag at `point` lands. The queue goes first, its floating card sitting over the content.
pub(super) fn route(point: LogicalPosition, targets: &DropTargets) -> DropRoute {
    if targets.queue.as_ref().is_some_and(|rect| contains(rect, point)) {
        return DropRoute::Queue;
    }
    match &targets.playlist {
        Some((rect, id)) if contains(rect, point) => DropRoute::Playlist(*id),
        _ => DropRoute::Nowhere,
    }
}

fn contains(rect: &WindowRect, point: LogicalPosition) -> bool {
    (rect.x..rect.x + rect.width).contains(&point.x)
        && (rect.y..rect.y + rect.height).contains(&point.y)
}

/// Starts the import a drop routed to `route` asks for. A set of files arrives in one event, so
/// one import serves it whole.
pub(super) fn import(state: &AppState, route: DropRoute, paths: &[PathBuf]) {
    let paths: Vec<String> = paths.iter().map(|path| path.to_string_lossy().into_owned()).collect();
    let runtime = state.runtime.clone();
    let state = state.clone();
    match route {
        DropRoute::Queue => {
            runtime.spawn(async move { import_to_queue(&state, paths).await });
        }
        DropRoute::Playlist(id) => {
            runtime.spawn(async move { import_to_playlist(&state, id, paths).await });
        }
        DropRoute::Nowhere => {}
    }
}

async fn import_to_queue(state: &AppState, paths: Vec<String>) {
    match library::queue::queue_import_files(state, paths).await {
        // The queue itself updates through `sinks.queue`, but no other view sees freshly-imported
        // tracks without this bump.
        Ok(_) => state.library_changed.bump(),
        Err(e) => {
            log::warn!("queue_import_files (drop): {}", describe(&e));
            toast::notify(ToastKind::OperationFailed, describe(&e));
        }
    }
}

async fn import_to_playlist(state: &AppState, playlist_id: i64, paths: Vec<String>) {
    match library::playlists::import_files_to_playlist(state, playlist_id, paths).await {
        // Refreshes the open detail's track list *and* the grid, where a previously-empty playlist
        // has a new count and thumbnail.
        Ok(_) => state.library_changed.bump(),
        Err(e) => log::warn!("import_files_to_playlist (drop): {}", describe(&e)),
    }
}

#[cfg(test)]
#[path = "tests/file_drop_tests.rs"]
mod tests;
