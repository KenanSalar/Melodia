//! What a card's right-click menu does, for every grid.
//!
//! Each handler is the same three steps — parse the kind, resolve the selection to track ids off
//! the event loop, act — so the resolve lives in [`spawn_with_track_ids`] and the handlers are the
//! act alone. Browse's folder cards are wired in its own slice: a folder is a path, not an id.

use std::future::Future;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::ui::callbacks::DialogClaim;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use melodia_app::library::{self, entity_tracks::EntityKind};
use melodia_app::state::AppState;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, CardActions, Dialog, Playlists, TagEditor};

/// Wire every `CardActions` callback.
pub fn wire(ui: &AppWindow, state: &AppState) {
    let actions = ui.global::<CardActions>();
    wire_transport(&actions, state);
    wire_set_favorites(&actions, state);
    wire_create_playlist_from(ui, state);
    wire_add_to_playlist(ui, state);
    wire_edit_tags(ui, state);
}

/// Play, Shuffle, Play Next and Add to Queue.
fn wire_transport(actions: &CardActions<'_>, state: &AppState) {
    {
        let state = state.clone();
        actions.on_play(move |kind, ids| {
            spawn_with_track_ids(&state, "card play", &kind, &ids, |state, track_ids| async move {
                library::playback::player_play_tracks(&state.playback_ctx(), track_ids, Some(0))
                    .await
            });
        });
    }
    {
        let state = state.clone();
        actions.on_shuffle(move |kind, ids| {
            spawn_with_track_ids(
                &state,
                "card shuffle",
                &kind,
                &ids,
                |state, track_ids| async move {
                    // Spawns its own task; reused rather than re-derived so the random start slot and
                    // the set-don't-toggle rule stay stated once.
                    super::spawn_play_then_shuffle(&state, "card shuffle", track_ids);
                    Ok(())
                },
            );
        });
    }
    {
        let state = state.clone();
        actions.on_play_next(move |kind, ids| {
            spawn_with_track_ids(
                &state,
                "card play next",
                &kind,
                &ids,
                |state, track_ids| async move {
                    library::queue::queue_play_next_many(&state, track_ids).await
                },
            );
        });
    }
    {
        let state = state.clone();
        actions.on_add_to_queue(move |kind, ids| {
            spawn_with_track_ids(
                &state,
                "card add to queue",
                &kind,
                &ids,
                |state, track_ids| async move {
                    library::queue::queue_add_tracks(&state, track_ids).await
                },
            );
        });
    }
}

fn wire_set_favorites(actions: &CardActions<'_>, state: &AppState) {
    let state = state.clone();
    actions.on_set_favorites(move |kind, ids, favorite| {
        spawn_with_track_ids(
            &state,
            "card favorite",
            &kind,
            &ids,
            move |state, track_ids| async move {
                library::favorites::set_favorite(&state, track_ids, favorite).await
            },
        );
    });
}

fn wire_create_playlist_from(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<CardActions>().on_create_playlist_from(move |kind, ids| {
        // The opener sets its own `@tr` chrome and raises the card, so there is nothing to
        // fill here. Safe to flip `Dialog.open` from this hop: the recursion guard is about
        // doing it *inside* the click handler, and this is a later tick.
        open_with_track_ids(&state, "card new playlist", &kind, &ids, &weak, |ui, track_ids| {
            ui.global::<Dialog>().invoke_open_create_playlist(to_id_model(&track_ids));
        });
    });
}

fn wire_add_to_playlist(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<CardActions>().on_add_to_playlist(move |kind, ids| {
        open_with_track_ids(&state, "card add to playlist", &kind, &ids, &weak, |ui, track_ids| {
            // The menu set the chrome before calling, `@tr` resolving literals at codegen.
            // What it could not know is the track count behind the cards.
            let model = to_id_model(&track_ids);
            let dialog = ui.global::<Dialog>();
            dialog.set_pending_track_ids(model.clone());
            dialog.set_pick_total_tracks(len_as_i32(track_ids.len()));
            // No playlist to exclude: a grid card is never the playlist you are inside.
            ui.global::<Playlists>().invoke_request_add_to_playlist(model, -1);
        });
    });
}

fn wire_edit_tags(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<CardActions>().on_edit_tags(move |kind, ids, title| {
        open_with_track_ids(&state, "card edit tags", &kind, &ids, &weak, move |ui, track_ids| {
            let tab = ui.global::<TagEditor>().get_tab_tags();
            ui.global::<Dialog>().invoke_open_tag_editor(title, to_id_model(&track_ids), tab);
        });
    });
}

/// Parse the kind, resolve `entity_ids` to track ids on the runtime, and hand them to `act`,
/// logging a failure under `label`.
///
/// Bails on an unknown kind rather than guessing one, and on an empty result rather than asking
/// the transport to play nothing — an entity whose tracks were removed under the open grid.
fn spawn_with_track_ids<Fut>(
    state: &AppState,
    label: &'static str,
    kind: &SharedString,
    entity_ids: &ModelRc<i32>,
    act: impl FnOnce(AppState, Vec<i64>) -> Fut + Send + 'static,
) where
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    let Some(kind) = EntityKind::from_token(kind.as_str()) else {
        log::warn!("{label}: unknown card kind {kind}");
        return;
    };
    let entity_ids: Vec<i64> = entity_ids.iter().map(i64::from).collect();
    if entity_ids.is_empty() {
        return;
    }

    let state = state.clone();
    state.runtime.clone().spawn(async move {
        match library::entity_tracks::track_ids_for(&state.db, kind, &entity_ids).await {
            Ok(track_ids) if track_ids.is_empty() => {
                log::debug!("{label}: nothing behind the selection");
            }
            Ok(track_ids) => {
                if let Err(e) = act(state, track_ids).await {
                    log::warn!("{label}: {}", describe(&e));
                }
            }
            Err(e) => log::warn!("{label}: {}", describe(&e)),
        }
    });
}

/// [`spawn_with_track_ids`] for a menu entry that raises a dialog: the claim is taken at the
/// click, and `open` runs on the UI thread only while it still holds.
fn open_with_track_ids(
    state: &AppState,
    label: &'static str,
    kind: &SharedString,
    entity_ids: &ModelRc<i32>,
    weak: &Weak<AppWindow>,
    open: impl FnOnce(&AppWindow, Vec<i64>) + Send + 'static,
) {
    let Some(claim) = DialogClaim::take_from(weak) else { return };
    let weak = weak.clone();
    spawn_with_track_ids(state, label, kind, entity_ids, move |_, track_ids| async move {
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if claim.holds(&ui) {
                open(&ui, track_ids);
            }
        });
        Ok(())
    });
}

/// Track ids as the `[int]` model every dialog handoff takes.
fn to_id_model(track_ids: &[i64]) -> ModelRc<i32> {
    ModelRc::new(VecModel::from(
        track_ids.iter().copied().map(clamp_i64_to_i32).collect::<Vec<i32>>(),
    ))
}
