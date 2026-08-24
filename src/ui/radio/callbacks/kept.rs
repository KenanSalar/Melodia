//! The kept tabs' own wiring: the sort, the add / edit form, and remove.
//!
//! Import and export are not here — their completion toasts need the notifications stack, which
//! does not exist yet when a slice installs, so they wire from `main()` through
//! [`super::files`], the shape `ui::playlists::wire_files` already uses.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString, Weak};

use crate::entities::radio;
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
        g.on_remove_from_favorites(move |id| {
            remove(&s, &ru, &weak, i64::from(id), RemoveFrom::Favorites);
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_remove_from_recent(move |id| {
            remove(&s, &ru, &weak, i64::from(id), RemoveFrom::Recent);
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

/// Which list the trash was clicked on, and so what removal means there.
#[derive(Clone, Copy)]
enum RemoveFrom {
    Favorites,
    Recent,
}

/// Drop a station out of one tab, then re-read both lists.
///
/// Whether the row itself survives is the facade's call — it knows what the other tab still holds
/// — so all this has to carry is which list asked.
fn remove(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &Weak<AppWindow>,
    id: i64,
    from: RemoveFrom,
) {
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        let removed = match from {
            RemoveFrom::Favorites => library::radio::remove_from_favorites(&s, id).await,
            RemoveFrom::Recent => library::radio::remove_from_recent(&s, id).await,
        };
        if let Err(e) = removed {
            log::warn!("radio: station removal failed: {}", crate::services::describe(&e));
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| kept::refresh(&ui, &s, &ru));
    });
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
        // Asked before the fields below move out of the row. Which of the four the form offers is
        // the entity's call, not the dialog's — the rule is one misclick away from being wrong.
        let (can_website, can_logo) = (station.can_set_website(), station.can_set_logo());
        let (can_genre, can_country) = (station.can_set_genre(), station.can_set_country());
        // The uuid is what the directory identifies the row by, so carrying one is what makes the
        // name and stream URL the directory's rather than the user's.
        let directory_owned = station.station_uuid.is_some();
        let form = ui.global::<RadioForm>();
        form.set_edit_id(crate::ui::util::clamp_i64_to_i32(station.id));
        form.set_url(SharedString::from(&station.stream_url));
        form.set_name(SharedString::from(&station.name));
        // The user's own answers, never the resolved ones: these fields are what they may change,
        // and seeding from `website()` or `genre()` would offer a directory value up for editing
        // by way of the save that follows.
        form.set_website(SharedString::from(station.local_homepage.unwrap_or_default()));
        form.set_logo_url(SharedString::from(station.local_favicon_url.unwrap_or_default()));
        form.set_genre(SharedString::from(station.local_tags.unwrap_or_default()));
        form.set_country(SharedString::from(station.local_country.unwrap_or_default()));
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
        dialog.set_kind(SharedString::from("edit-station"));
        dialog.set_target_id(crate::ui::util::clamp_i64_to_i32(station.id));
        dialog.set_open(true);
    });
}

/// Validate and save whatever the form holds, closing the dialog only once it worked.
///
/// **A blank field means different things in the two modes**, hence the split guard: an empty
/// stream URL is nothing to save, where an empty website is the user clearing the link.
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
                    // A station the user just described is worth one more look for a logo, even
                    // where this session already gave up on it: the site they named is new
                    // evidence, and the repair skips an id it has tried.
                    ru.forget_heal(id);
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

/// Write whatever the form is for, and the user's own fields with it. Returns the station's id.
///
/// **Two calls rather than a wider `update_custom_station`**, because they answer to different
/// owners: the station's own fields are re-derived from the stream whenever its URL moves, where
/// the four overrides are the user's and survive that. Folding them into the probe path is what
/// would put them back in the directory's columns.
///
/// The overrides go **last**, so a refused URL leaves nothing half-written on a station that was
/// otherwise saved — the form stays open on the error and the retry re-runs both.
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

/// Which of the form's four localized lines an error deserves.
///
/// Resolved through the `err-*` callbacks rather than built in Rust: `@tr` folds msgids at
/// codegen, so a sentence Rust pushed would render untranslated. The split is by stage — the
/// address, the connection, the decode, or the write.
fn form_error(form: &RadioForm<'_>, error: &AppError) -> SharedString {
    match error {
        // Either typed URL's own refusal. `ensure_editable` raises the same variant and its line
        // would read wrong here, but it cannot reach this form: a directory-owned row submits
        // through `set_station_overrides`, which never asks it.
        AppError::Validation(_) => form.invoke_err_bad_url(),
        AppError::Network { .. } => form.invoke_err_unreachable(),
        AppError::Database(_) | AppError::Io(_) => form.invoke_err_save_failed(),
        // `Player` is the decoder's own refusal and is what this arm is for.
        _ => form.invoke_err_not_audio(),
    }
}
