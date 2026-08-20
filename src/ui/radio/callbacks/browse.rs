//! The directory grid's wiring: what a column change, a load-more, a card and a logo lookup do.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::radio::{RadioUi, browse};
use crate::ui::{grid_prewarm, launcher};
use crate::{AppWindow, Radio};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    {
        // A resize re-chunks the same stations; nothing is re-fetched, the column count being a
        // property of the window rather than of the query.
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_columns_changed(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            browse::apply(&ui, &ru);
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_load_more(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse::load_more(&ui, &s, &ru);
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_retry(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse::retry(&ui, &s, &ru);
        });
    }

    {
        // The card's lazy logo lookup. `grid_cover` is the branch every grid takes: cache-only
        // while the generation is `0`, scheduling past it, and never decoding on this thread.
        let ru = radio_ui.clone();
        g.on_request_logo(move |artwork_path, generation| {
            grid_prewarm::grid_cover(&ru.covers, &artwork_path, generation)
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        g.on_play_station(move |row| {
            let Some((station, logo)) = browse::resolve(&ru, &row.uuid) else {
                return;
            };
            let s2 = s.clone();
            crate::ui::callbacks::macros::spawn_logged!(
                s,
                "radio::play_station",
                library::radio::play_directory_station(&s2, &station, logo.as_deref())
            );
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_toggle_favorite(move |row| {
            let Some(ui) = weak.upgrade() else { return };
            let Some((station, logo)) = browse::resolve(&ru, &row.uuid) else {
                return;
            };
            let uuid = row.uuid.to_string();
            let wanted = !row.is_favorite;

            // Optimistic, like every other row flag in the tree: a star is not list membership
            // on this tab, so nothing has to be re-fetched for it to be right.
            ru.set_local_favorite(&uuid, wanted);
            browse::apply(&ui, &ru);

            let (s2, ru2, weak2) = (s.clone(), ru.clone(), weak.clone());
            s.runtime.spawn(async move {
                let Err(e) =
                    library::radio::set_directory_favorite(&s2, &station, wanted, logo.as_deref())
                        .await
                else {
                    return;
                };
                log::warn!("radio: favorite toggle failed: {}", crate::services::describe(&e));
                // Put the star back rather than leaving it claiming a row that was never
                // written. A routine failure, so it is a log line and not a toast.
                ru2.set_local_favorite(&uuid, !wanted);
                let _ = weak2.upgrade_in_event_loop(move |ui| browse::apply(&ui, &ru2));
            });
        });
    }

    {
        let s = state.clone();
        g.on_open_homepage(move |url| {
            if url.is_empty() {
                return;
            }
            s.runtime.spawn(launcher::open_target(url.to_string(), "radio::open_homepage"));
        });
    }
}
