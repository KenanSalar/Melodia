//! What a folder card's right-click menu does.
//!
//! Here rather than on the shared `CardActions` global because a folder is named by a path, and
//! every callback there carries an id list. All three transport arms resolve through
//! [`library::browse::folder_track_ids`], which is **recursive**: a folder card is one the user
//! has not navigated into, so an artist folder holding only album subfolders would otherwise offer
//! a Play that queues nothing.

use std::future::Future;

use slint::{ComponentHandle, SharedString};

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Browse};

/// Wire the four folder-card callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Browse>();

    {
        let state = state.clone();
        g.on_play_folder(move |path| {
            spawn_with_folder_tracks(
                &state,
                "browse play folder",
                &path,
                |state, ids| async move {
                    let played =
                        library::playback::player_play_tracks(&state.playback_ctx(), ids, Some(0))
                            .await;
                    if let Err(e) = played {
                        log::warn!("browse play folder: {}", describe(&e));
                    }
                },
            );
        });
    }

    {
        let state = state.clone();
        g.on_play_folder_next(move |path| {
            spawn_with_folder_tracks(
                &state,
                "browse folder next",
                &path,
                |state, ids| async move {
                    if let Err(e) = library::queue::queue_play_next_many(&state, ids).await {
                        log::warn!("browse folder next: {}", describe(&e));
                    }
                },
            );
        });
    }

    {
        let state = state.clone();
        g.on_queue_folder(move |path| {
            spawn_with_folder_tracks(
                &state,
                "browse queue folder",
                &path,
                |state, ids| async move {
                    if let Err(e) = library::queue::queue_add_tracks(&state, ids).await {
                        log::warn!("browse queue folder: {}", describe(&e));
                    }
                },
            );
        });
    }

    {
        let state = state.clone();
        g.on_reveal_folder(move |path| {
            let folder = std::path::PathBuf::from(path.as_str());
            let state = state.clone();
            state.runtime.clone().spawn(async move {
                if let Err(e) = library::tracks::reveal_folder(folder).await {
                    log::warn!("browse reveal folder: {}", describe(&e));
                }
            });
        });
    }
}

/// Resolve `path` to its track ids on the runtime, then hand them to `act`.
///
/// Bails on an empty result rather than asking the transport to play nothing — a folder holding
/// only files the scanner never took.
fn spawn_with_folder_tracks<Fut>(
    state: &AppState,
    label: &'static str,
    path: &SharedString,
    act: impl FnOnce(AppState, Vec<i64>) -> Fut + Send + 'static,
) where
    Fut: Future<Output = ()> + Send + 'static,
{
    let path = path.to_string();
    let state = state.clone();
    state.runtime.clone().spawn(async move {
        match library::browse::folder_track_ids(&state, &path).await {
            Ok(ids) if ids.is_empty() => log::debug!("{label}: no tracks under {path}"),
            Ok(ids) => act(state, ids).await,
            Err(e) => log::warn!("{label}: {}", describe(&e)),
        }
    });
}
