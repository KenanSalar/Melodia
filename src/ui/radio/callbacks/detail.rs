//! The station page's own callbacks: opening it, closing it, and the two actions no card carries.
//!
//! The row actions the page shares with every card — play, star, homepage, edit, remove — stay in
//! [`super::stations`] and [`super::kept`], reached from the detail body through the same
//! `Radio.*` callbacks a card fires. The page repoints nothing.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::{release_hero_slots, release_shared_hero};
use crate::utils::toast::{self, ToastKind};
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
            let (s, ru, weak) = (s.clone(), ru.clone(), weak.clone());
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    detail::open_station(&s, &ru, weak, station, NavEnterFrom::Below).await
                {
                    log::warn!("radio: open station: {}", crate::error::describe(&e));
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

            // Giving up the seat is what closes it: `detail-open` is derived from this.
            ui.global::<Radio>().set_detail_tab(detail::NO_SEAT);
            // The hero's images are deliberately left alone: the band paints them all the way
            // through the collapse, and `hero-collapsed` is what hands them back at the end.
            // **The mounted tab's page alone** — the back arrow is one page's, and the other tabs
            // are still holding theirs.
            detail::close_detail(&ui, &ru);
            detail::persist_seat(&s, &ui, &ru);

            // The tier stays where another tab is still holding a page: its hero is decoded out
            // of it and a return trip repaints from that.
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
            // A tab pick collapses the band too, over a tab that simply has no page. What the
            // *other* tabs are holding is safe either way: their heroes live in their seats now,
            // so these slots only ever carry the mounted page's.
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
        g.on_vote(move |uuid, station_id| {
            if uuid.is_empty() {
                return;
            }
            let station = detail::StationRef {
                id: i64::from(station_id),
                uuid: uuid.to_string(),
            };
            let (s, ru, weak) = (s.clone(), ru.clone(), weak.clone());
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::radio::vote(&s, &station.uuid).await {
                    toast::notify(ToastKind::RadioVote, crate::error::describe(&e));
                    return;
                }
                // Re-read rather than adding one locally: the server deduplicates, so a local
                // bump would state a total the directory does not have. Against the station the
                // click captured, the tab being free to move under a request in flight — and a
                // no-op where no tab is holding it, which is every vote cast from Now Playing on
                // a station whose page is not open.
                detail::refresh_from_directory(&s, &ru, &weak, &station).await;
            });
        });
    }

    // Slint's `TextInput` owns the clipboard write; `StationFacts` mounts a zero-sized one over
    // the URL it draws and calls `select-all()` / `copy()`. Nothing is owed here but the log line
    // that says a station's URL was taken — and not the URL itself, which a station can carry a
    // session token in.
    g.on_copy_stream_url(|station_id| {
        log::debug!("radio: stream URL copied for station {station_id}");
    });
}
