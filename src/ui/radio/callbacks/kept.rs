//! The kept tabs' own wiring: the sort, the add / edit form, and remove.
//!
//! Import and export are not here — their completion toasts need the notifications stack, which
//! does not exist yet when a slice installs, so they wire from `main()` through
//! [`super::files`], the shape `ui::playlists::wire_files` already uses.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString, Weak};

use crate::error::AppError;
use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::{next_sort, persist_view_sort, persisted_sort};
use crate::ui::radio::{RadioTab, RadioUi, kept};
use crate::ui::track_list_view::view_id;
use crate::{AppWindow, Dialog, Radio, RadioForm};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    // The persisted sort, ahead of the first apply. A fresh install keeps the Slint-declared
    // default rather than being handed one from Rust.
    if let Some((field, dir)) = persisted_sort(state, view_id::RADIO_FAVORITES) {
        g.set_sort_field(SharedString::from(field.as_str()));
        g.set_sort_dir(SharedString::from(dir));
    }

    {
        // Same field flips the direction, a new field starts ascending — both through the shared
        // `next_sort`, so the arrow and the comparator cannot disagree.
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Radio>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            kept::apply(&ui, &ru, RadioTab::Favorites);
            persist_view_sort(&s, view_id::RADIO_FAVORITES, new_field, new_dir);
        });
    }

    {
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_edit_station(move |row| {
            open_editor(&ru, &weak, i64::from(row.id));
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_remove_station(move |id| {
            let (s, ru, weak) = (s.clone(), ru.clone(), weak.clone());
            let id = i64::from(id);
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::radio::remove_station(&s, id).await {
                    log::warn!("radio::remove_station: {}", crate::services::describe(&e));
                    return;
                }
                let _ = weak.upgrade_in_event_loop(move |ui| kept::refresh(&ui, &s, &ru));
            });
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        ui.global::<RadioForm>().on_submit(move || {
            let Some(ui) = weak.upgrade() else { return };
            submit(&ui, &s, &ru, &weak);
        });
    }
}

/// Fill the form from the station behind a card and open the dialog on it.
///
/// **From a fresh event-loop tick**, like every other Rust-side dialog open: writing `Dialog`
/// synchronously inside a click handler re-enters Slint's property evaluator and trips its
/// recursion guard.
fn open_editor(radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>, id: i64) {
    // Either tab can hold the row — the same station is in both once it has been played.
    let Some(station) = kept::resolve(radio_ui, RadioTab::Favorites, id)
        .or_else(|| kept::resolve(radio_ui, RadioTab::Recent, id))
    else {
        return;
    };

    let _ = weak.upgrade_in_event_loop(move |ui| {
        let form = ui.global::<RadioForm>();
        form.set_edit_id(crate::ui::util::clamp_i64_to_i32(station.id));
        form.set_url(SharedString::from(&station.stream_url));
        form.set_name(SharedString::from(&station.name));
        form.set_busy(false);
        form.set_error(SharedString::default());

        let dialog = ui.global::<Dialog>();
        dialog.set_title(form.invoke_edit_title());
        dialog.set_message(SharedString::default());
        dialog.set_confirm_label(form.invoke_save_label());
        dialog.set_cancel_label(form.invoke_cancel_label());
        dialog.set_destructive(false);
        dialog.set_kind(SharedString::from("edit-station"));
        dialog.set_target_id(crate::ui::util::clamp_i64_to_i32(station.id));
        dialog.set_open(true);
    });
}

/// Validate and save whatever the form holds, closing the dialog only once it worked.
fn submit(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>) {
    let form = ui.global::<RadioForm>();
    let (url, name, edit_id) =
        (form.get_url().to_string(), form.get_name().to_string(), form.get_edit_id());
    if url.trim().is_empty() || form.get_busy() {
        return;
    }
    form.set_busy(true);
    form.set_error(SharedString::default());

    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        let outcome = if edit_id < 0 {
            library::radio::add_custom_station(&s, url.trim(), &name).await.map(|_id| ())
        } else {
            library::radio::update_custom_station(&s, i64::from(edit_id), url.trim(), &name).await
        };

        let _ = weak.upgrade_in_event_loop(move |ui| {
            let form = ui.global::<RadioForm>();
            form.set_busy(false);
            match outcome {
                Ok(()) => {
                    ui.global::<Dialog>().set_open(false);
                    kept::refresh(&ui, &s, &ru);
                }
                Err(e) => {
                    log::warn!("radio: station form: {}", crate::services::describe(&e));
                    form.set_error(form_error(&form, &e));
                }
            }
        });
    });
}

/// Which of the form's three localized lines an error deserves.
///
/// Resolved through the `err-*` callbacks rather than built in Rust: `@tr` folds msgids at
/// codegen, so a sentence Rust pushed would render untranslated. The split is by stage —
/// the connection, the decode, or the write — and each variant belongs to exactly one.
fn form_error(form: &RadioForm<'_>, error: &AppError) -> SharedString {
    match error {
        AppError::Network { .. } => form.invoke_err_unreachable(),
        AppError::Database(_) | AppError::Io(_) => form.invoke_err_save_failed(),
        // `Player` is what the stream decoder refuses with, `Validation` what the facade does.
        _ => form.invoke_err_not_audio(),
    }
}
