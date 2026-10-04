//! `CopyActions`, the Copy entries the row and card menus offer.
//!
//! Each parses its field, resolves the text off the event loop through `library::clipboard`, and
//! lands it through [`crate::ui::clipboard::write`]. Stations copy through `Radio` instead, the
//! radio slice owning the caches they resolve from.

use std::future::Future;

use slint::{ComponentHandle, Model, Weak};

use crate::ui::callbacks::collect_nonzero_track_ids;
use crate::ui::clipboard;
use melodia_app::library::clipboard::{EntityField, TrackField, entity_lines, track_lines};
use melodia_app::library::entity_tracks::EntityKind;
use melodia_app::state::AppState;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::toast::{self, ToastKind};
use melodia_ui::{AppWindow, CopyActions};

/// Wire every `CopyActions` callback.
pub fn wire(ui: &AppWindow, state: &AppState) {
    let actions = ui.global::<CopyActions>();

    {
        let weak = ui.as_weak();
        actions.on_copy_text(move |text| {
            if let Some(ui) = weak.upgrade() {
                clipboard::write(&ui, &text);
            }
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        actions.on_copy_tracks(move |field, ids| {
            let Some(field) = TrackField::from_token(&field) else {
                log::warn!("copy: unknown track field {field}");
                return;
            };
            let ids = collect_nonzero_track_ids(&ids);
            spawn_copy(&state, &weak, "copy tracks", move |state| async move {
                track_lines(&state, &ids, field).await
            });
        });
    }

    {
        let state = state.clone();
        let weak = ui.as_weak();
        actions.on_copy_entities(move |kind, field, ids| {
            let (Some(kind), Some(field)) =
                (EntityKind::from_token(&kind), EntityField::from_token(&field))
            else {
                log::warn!("copy: unknown card kind {kind} or field {field}");
                return;
            };
            let ids: Vec<i64> = ids.iter().map(i64::from).collect();
            spawn_copy(&state, &weak, "copy cards", move |state| async move {
                entity_lines(&state, kind, &ids, field).await
            });
        });
    }
}

/// Resolve the text on the runtime and hand it back to the event loop to copy.
fn spawn_copy<Fut>(
    state: &AppState,
    weak: &Weak<AppWindow>,
    label: &'static str,
    resolve: impl FnOnce(AppState) -> Fut + Send + 'static,
) where
    Fut: Future<Output = Result<String, AppError>> + Send + 'static,
{
    let weak = weak.clone();
    let state = state.clone();
    state.runtime.clone().spawn(async move {
        match resolve(state).await {
            Ok(text) => {
                let _ = weak.upgrade_in_event_loop(move |ui| clipboard::write(&ui, &text));
            }
            Err(e) => {
                log::warn!("{label}: {}", describe(&e));
                // The menu closed on the click, so a failure owes a sign as much as a copy does.
                toast::notify(ToastKind::OperationFailed, e.to_string());
            }
        }
    });
}
