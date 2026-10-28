//! The station page's own callbacks: opening it, closing it, and the two actions no card carries.
//! The row actions it shares with every card stay in [`super::stations`] and [`super::kept`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::detail_view::{release_hero_slots, release_shared_hero};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::utils::toast::{self, ToastKind};
use melodia_ui::{AppWindow, NavEnterFrom, Radio};

use super::super::{RadioUi, detail};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_open_station(move |row| {
            let station = detail::StationRef::from_row(&row);
            let (s, ru, weak) = (s.clone(), ru.clone(), weak.clone());
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    detail::open_station(&s, &ru, weak, station, NavEnterFrom::Above).await
                {
                    log::warn!("radio: open station: {}", melodia_core::error::describe(&e));
                }
            });
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_close_detail(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Inert here, the page holding still as the band collapses, but every close spells it.
            crate::ui::nav_transition::mark_drill_back(&ui);

            // Giving up the seat is what closes it: `detail-open` is derived from this.
            ui.global::<Radio>().set_detail_tab(detail::NO_SEAT);
            // The hero's images stay for the collapse, `hero-collapsed` handing them back.
            detail::close_detail(&ui, &ru);
            detail::persist_seat(&s, &ui, &ru);

            // The tier stays while another tab holds a page decoded out of it.
            if !detail::any_seated(&ru) {
                let ru_swap = ru.clone();
                s.runtime.spawn_blocking(move || ru_swap.release_detail_artwork());
            }

            crate::ui::nav_history::record_current(&ui);
        });
    }

    {
        let weak = weak.clone();
        g.on_hero_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Radio>();
            // A tab pick collapses the band too. The other tabs' heroes live in their seats, so
            // these slots carry only the mounted page's.
            if g.get_detail_open() {
                return;
            }
            // Not `release_detail_hero_images`, whose gate asks about My Library's band.
            release_hero_slots(&g);
            release_shared_hero(&ui);
        });
    }

    {
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_detail_scope_changed(move || {
            let Some(ui) = weak.upgrade() else { return };
            super::super::filter::sync_box(&ui, &ru);
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_vote(move |uuid, station_id| {
            if uuid.is_empty() {
                return;
            }
            let station = detail::StationRef { id: i64::from(station_id), uuid: uuid.to_string() };
            let (s, ru, weak) = (s.clone(), ru.clone(), weak.clone());
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::radio::vote(&s, &station.uuid).await {
                    toast::notify(ToastKind::RadioVote, melodia_core::error::describe(&e));
                    return;
                }
                // Re-read rather than bumped locally, the server deduplicating. Against the
                // station the click captured, a no-op where no tab holds it.
                detail::refresh_from_directory(&s, &ru, &weak, &station).await;
            });
        });
    }
}
