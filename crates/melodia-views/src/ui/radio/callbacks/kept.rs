//! The kept tabs' own wiring: the sort and the add / edit form. Removal is [`super::removal`]'s,
//! import and export [`super::files`]'s.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString, Weak};

use crate::ui::callbacks::{next_sort, persist_view_sort, persisted_sort};
use crate::ui::radio::{RadioTab, RadioUi, kept};
use crate::ui::track_list_view::view_id;
use crate::ui::util::opt_shared;
use melodia_app::library;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_core::entities::radio;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, Dialog, DialogKind, Radio, RadioForm};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    radio_ui: &Arc<RadioUi>,
) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    // The persisted sort, ahead of the first apply. A fresh install keeps the Slint-declared
    // default rather than being handed one from Rust.
    if let Some(sort) = persisted_sort(view_state, view_id::RADIO_FAVORITES) {
        g.set_sort_field(SharedString::from(sort.field.as_str()));
        g.set_sort_dir(SharedString::from(sort.dir.as_str()));
    }

    {
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
        ui.global::<RadioForm>().on_submit(move || {
            let Some(ui) = weak.upgrade() else { return };
            submit(&ui, &s, &ru, &weak);
        });
    }
}

/// Fills the form from a card's station and opens the dialog, from a fresh event-loop tick: a
/// synchronous `Dialog` write in a click handler trips Slint's recursion guard.
fn open_editor(radio_ui: &RadioUi, weak: &Weak<AppWindow>, id: i64) {
    // Either tab can hold the row — the same station is in both once it has been played.
    let Some(station) = kept::resolve(radio_ui, RadioTab::Favorites, id)
        .or_else(|| kept::resolve(radio_ui, RadioTab::Recent, id))
    else {
        return;
    };

    let _ = weak.upgrade_in_event_loop(move |ui| {
        // Before the fields move out of the row; which of the four the form offers is the entity's
        // call.
        let (can_website, can_logo) = (station.can_set_website(), station.can_set_logo());
        let (can_genre, can_country) = (station.can_set_genre(), station.can_set_country());
        // The uuid is what the directory identifies the row by, so carrying one is what makes the
        // name and stream URL the directory's rather than the user's.
        let directory_owned = station.station_uuid.is_some();
        let form = ui.global::<RadioForm>();
        form.set_edit_id(crate::ui::util::clamp_i64_to_i32(station.id));
        form.set_url(SharedString::from(&station.stream_url));
        form.set_name(SharedString::from(&station.name));
        // The user's own answers, never the resolved ones, or a save would copy a directory value
        // into an override.
        form.set_website(opt_shared(station.local_homepage));
        form.set_logo_url(opt_shared(station.local_favicon_url));
        form.set_genre(opt_shared(station.local_tags));
        form.set_country(opt_shared(station.local_country));
        form.set_can_edit_website(can_website);
        form.set_can_edit_logo(can_logo);
        form.set_can_edit_genre(can_genre);
        form.set_can_edit_country(can_country);
        form.set_directory_owned(directory_owned);
        form.set_busy(false);
        form.set_error(SharedString::default());

        let dialog = ui.global::<Dialog>();
        dialog.set_title(if directory_owned {
            form.invoke_details_title()
        } else {
            form.invoke_edit_title()
        });
        dialog.set_message(SharedString::default());
        dialog.set_confirm_label(form.invoke_save_label());
        dialog.set_cancel_label(form.invoke_cancel_label());
        dialog.set_destructive(false);
        dialog.set_kind(DialogKind::EditStation);
        dialog.set_target_id(crate::ui::util::clamp_i64_to_i32(station.id));
        dialog.set_open(true);
    });
}

/// Validates and saves the form, closing the dialog only once it worked. An empty stream URL is
/// nothing to save, where an empty website is the user clearing the link.
fn submit(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>) {
    let form = ui.global::<RadioForm>();
    let (url, name, edit_id) =
        (form.get_url().to_string(), form.get_name().to_string(), form.get_edit_id());
    let directory_owned = form.get_directory_owned();
    let overrides = radio::StationOverrides {
        website: Some(form.get_website().to_string()),
        logo_url: Some(form.get_logo_url().to_string()),
        genre: Some(form.get_genre().to_string()),
        country: Some(form.get_country().to_string()),
    };
    if form.get_busy() || (!directory_owned && url.trim().is_empty()) {
        return;
    }
    form.set_busy(true);
    form.set_error(SharedString::default());

    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        let outcome = save_form(&s, edit_id, directory_owned, url.trim(), &name, &overrides).await;

        let _ = weak.upgrade_in_event_loop(move |ui| {
            let form = ui.global::<RadioForm>();
            form.set_busy(false);
            match outcome {
                Ok(id) => {
                    ui.global::<Dialog>().set_open(false);
                    // The site the user just named is new evidence for a logo.
                    ru.forget_heal(id);
                    kept::refresh(&ui, &s, &ru);
                }
                Err(e) => {
                    log::warn!("radio: station form: {}", melodia_core::error::describe(&e));
                    form.set_error(form_error(&form, &e));
                }
            }
        });
    });
}

/// Writes whatever the form is for, then the user's own fields, returning the station's id. Two
/// calls because the station's fields are re-derived whenever its URL moves while the overrides
/// survive that; the overrides go last, so a refused URL leaves nothing half-written.
async fn save_form(
    state: &AppState,
    edit_id: i32,
    directory_owned: bool,
    url: &str,
    name: &str,
    overrides: &radio::StationOverrides,
) -> Result<i64, AppError> {
    if directory_owned {
        let id = i64::from(edit_id);
        library::radio::set_station_overrides(state, id, overrides).await?;
        return Ok(id);
    }
    let id = if edit_id < 0 {
        library::radio::add_custom_station(state, url, name).await?
    } else {
        let id = i64::from(edit_id);
        library::radio::update_custom_station(state, id, url, name).await?;
        id
    };
    library::radio::set_station_overrides(state, id, overrides).await?;
    Ok(id)
}

/// Which of the form's four localized lines an error deserves, through the `err-*` callbacks
/// because `@tr` resolves at codegen and a sentence Rust built would render untranslated.
fn form_error(form: &RadioForm<'_>, error: &AppError) -> SharedString {
    match error {
        // A typed URL's refusal. `ensure_editable`'s never reaches this form, a directory-owned
        // row submitting through `set_station_overrides`.
        AppError::Validation { .. } => form.invoke_err_bad_url(),
        AppError::Network { .. } => form.invoke_err_unreachable(),
        AppError::Database(_) | AppError::Io(_) => form.invoke_err_save_failed(),
        // `Player` is the decoder's own refusal and is what this arm is for.
        _ => form.invoke_err_not_audio(),
    }
}
