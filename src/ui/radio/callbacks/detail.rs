//! The station page's own callbacks: opening it, closing it, and the two actions no card carries.
//!
//! The row actions the page shares with every card — play, star, homepage, edit, remove — stay in
//! [`super::stations`] and [`super::kept`], reached from the detail body through the same
//! `Radio.*` callbacks a card fires. The page repoints nothing.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::services::toast::{self, ToastKind};
use crate::state::AppState;
use crate::ui::callbacks::macros::{release_hero_slots, release_shared_hero};
use crate::ui::track_list_view::view_id;
use crate::{AppWindow, NavEnterFrom, Radio};

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
            // Only a station with a database row can be named for the next launch — a browsed one
            // is a directory answer with a shelf life, and `id == 0` is what says so. Written on
            // the way in rather than at the close, so a crash mid-visit still restores.
            persist_open_station(&s, &station);

            let (s, ru, weak) = (s.clone(), ru.clone(), weak.clone());
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    detail::open_station(&s, &ru, weak, station, NavEnterFrom::Below).await
                {
                    log::warn!("radio: open station: {}", crate::services::describe(&e));
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
            // A station page is a body of the page it was opened from, so it holds still while
            // the band collapses; this answers for nothing here today and is what every other
            // close spells, so a later cross-section entry finds it already right.
            crate::ui::nav_transition::mark_drill_back(&ui);

            ui.global::<Radio>().set_detail_open(false);
            // The hero's images are deliberately left alone: the band paints them all the way
            // through the collapse, and `hero-collapsed` is what hands them back at the end.
            detail::close_detail(&ru);

            let ru_swap = ru.clone();
            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                ru_swap.release_detail_artwork();
                if let Err(e) =
                    library::settings::set_last_detail_id(&s_disk, view_id::RADIO_DETAIL, None)
                {
                    log::warn!("radio::close_detail persist: {e}");
                }
            });

            crate::ui::nav_history::record_current(&s, &ui);
        });
    }

    {
        let weak = weak.clone();
        g.on_hero_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Radio>();
            if g.get_detail_open() {
                return;
            }
            // Not `release_detail_hero_images!`: its slot gate asks whether *My Library's* band
            // is up, which for a Radio collapse is a question about another page. The pair
            // underneath it is the same.
            release_hero_slots!(g);
            release_shared_hero!(ui);
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
        g.on_vote(move || {
            let Some(station) = detail::open_station_ref(&ru) else {
                return;
            };
            if station.uuid.is_empty() {
                return;
            }
            let (s, ru, weak) = (s.clone(), ru.clone(), weak.clone());
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::radio::vote(&s, &station.uuid).await {
                    toast::notify(ToastKind::RadioVote, crate::services::describe(&e));
                    return;
                }
                // Re-read rather than adding one locally: the server deduplicates, so a local
                // bump would state a total the directory does not have.
                detail::refresh_open_station(&s, &ru, &weak).await;
            });
        });
    }

    {
        let ru = radio_ui.clone();
        g.on_copy_stream_url(move || {
            // Slint's `TextInput` owns the clipboard write; the body mounts a zero-sized one over
            // `detail-stream-url` and calls `select-all()` / `copy()`. Nothing is owed here but
            // the log line that says a station's URL was taken — and not the URL itself, which a
            // station can carry a session token in.
            if let Some(station) = detail::open_station_ref(&ru) {
                log::debug!("radio: stream URL copied for station {}", station.id);
            }
        });
    }
}

/// Remember an open station for the next launch, where it has a row to be remembered by.
fn persist_open_station(state: &AppState, station: &detail::StationRef) {
    let id = station.id;
    let state = state.clone();
    let has_row = station.is_kept();
    state.runtime.clone().spawn_blocking(move || {
        let named = has_row.then_some(id);
        if let Err(e) = library::settings::set_last_detail_id(&state, view_id::RADIO_DETAIL, named)
        {
            log::warn!("radio::open_station persist: {e}");
        }
    });
}
