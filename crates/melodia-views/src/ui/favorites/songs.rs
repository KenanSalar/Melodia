//! Songs tab: the keyset loads, the sort and filter shadows, and the row patches.
//!
//! The Tracks view's shape over the favourites: a [`TrackRows`] holding the ids in the tab's sort
//! and filter, both answered in SQL, and the rows near the viewport. Its model is kept across a
//! tab switch, being a keyset and a few hundred rows, and handed back on the section leave.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::{FavoritesTab, FavoritesUi};
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::paged_rows::{Keyset, Load, Ticket};
use melodia_app::services::settings::ViewSort;
use melodia_app::state::AppState;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder};
use melodia_core::error::{AppResult, describe};
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{AppWindow, Favorites, TrackListRow as UiTrackListRow};

const TRACKS_LABEL: &str = "Favorites.tracks";

/// The active sort. Slint mirrors it in `Favorites.sort-*`, but this cache is the source of truth
/// — those properties are written from Rust, never back.
pub fn current_sort(fav_ui: &FavoritesUi) -> ViewSort {
    fav_ui.state().sort.lock().clone()
}

/// Read-and-return the active filter needle, folded by [`set_filter`] and ready to hand to a
/// `row_match` predicate.
pub fn current_filter(fav_ui: &FavoritesUi) -> Needle {
    fav_ui.state().filter.lock().clone()
}

/// Update the cached sort. Callers follow it with [`requery_tracks`] and a `set_view_sort`
/// persist, so the next launch lands on the same order.
pub fn set_sort(fav_ui: &FavoritesUi, field: String, dir: SortDir) {
    *fav_ui.state().sort.lock() = ViewSort { field, dir };
}

/// Update the cached filter needle, so the live-refresh subscriber can reload on a background
/// thread without reading the Slint global. Folded on the way in, so every reader shares one.
pub fn set_filter(fav_ui: &FavoritesUi, filter: &str) {
    *fav_ui.state().filter.lock() = fold_needle(filter);
}

/// Load the favourites' ids under the current sort and filter, with the rows they open on and
/// their covers warm, and show them, every row the list still holds read again. Runs on whichever
/// thread awaits it: a tokio worker from the section enter or a tab pick, the UI thread from the
/// live-refresh subscriber.
///
/// Every way of showing nothing re-arms the flag, the caller having consumed it before spawning
/// this: a failed query, a leave landing mid-flight, and a write [`show_tracks`] refuses. A load
/// another overtook leaves the list to that one.
pub async fn refresh_tracks(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
) -> AppResult<()> {
    let ticket = fav_ui.state().song_tickets.issue_reload();
    reload_tracks(state, fav_ui, weak, ticket).await
}

/// [`refresh_tracks`] under a ticket already issued, which a requery overtaking a refresh runs in
/// that one's place.
async fn reload_tracks(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    ticket: Ticket,
) -> AppResult<()> {
    let (order, needle) = current_query(fav_ui);
    let keyset =
        track_rows::load(state, TrackListScope::Favorites, order, &needle, &fav_ui.songs_scroll)
            .await
            .inspect_err(|_| fav_ui.mark_songs_dirty())?;

    // A leave landing while the query was in flight has already emptied the model, so showing
    // this would undo that teardown behind a view nobody can see.
    if !fav_ui.section_active() {
        fav_ui.mark_songs_dirty();
        return Ok(());
    }
    // Overtaken already: the newer load owns the list, and nothing this warms would be drawn.
    if !fav_ui.state().song_tickets.is_latest(ticket) {
        return Ok(());
    }
    track_rows::prewarm(&fav_ui.cover_thumbs, &keyset).await;
    // The decode burst is an `.await`, so the guard is asked again — after the slow part, because
    // before it the leave hasn't happened yet.
    if !fav_ui.section_active() {
        fav_ui.mark_songs_dirty();
        return Ok(());
    }

    let fav_ui = Arc::clone(fav_ui);
    let unfiltered = needle.is_empty();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        show_tracks(&ui, &fav_ui, ticket, keyset, |g, model| {
            track_rows::settle_reload(g, model, &fav_ui.state().song_tickets, ticket, unfiltered);
        });
    });
    Ok(())
}

/// Reload the ids after a header click or a keystroke. The rows the list holds are still current,
/// so it re-reads none of them and warms no covers, unless it overtakes a refresh, which it then
/// runs in that one's place.
///
/// Nothing is loaded while another tab is mounted: entering Songs clears the shared filter, and a
/// Songs tab left filtered is marked dirty on its way out, so the pick that brings it back
/// refreshes it anyway.
pub fn requery_tracks(state: &AppState, fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    if fav_ui.active_tab() != FavoritesTab::Songs {
        return;
    }
    let s = state.clone();
    let fu = Arc::clone(fav_ui);
    let weak = weak.clone();
    let ticket = match fav_ui.state().song_tickets.issue_requery() {
        Load::Requery(ticket) => ticket,
        Load::Reload(ticket) => {
            state.runtime.spawn(async move {
                if let Err(e) = reload_tracks(&s, &fu, &weak, ticket).await {
                    log::warn!("favorites::requery_tracks: {}", describe(&e));
                }
            });
            return;
        }
    };
    let (order, needle) = current_query(fav_ui);
    state.runtime.spawn(async move {
        match track_rows::load(&s, TrackListScope::Favorites, order, &needle, &fu.songs_scroll)
            .await
        {
            Ok(keyset) => {
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    show_tracks(&ui, &fu, ticket, keyset, |_, _| {});
                });
            }
            Err(e) => {
                fu.mark_songs_dirty();
                log::warn!("favorites::requery_tracks: {}", describe(&e));
            }
        }
    });
}

/// Patch the Songs row showing `id` after a favourite or rating write. Only that row is read
/// again, so scroll position and its neighbours stay put.
pub fn patch_row(
    weak: &Weak<AppWindow>,
    id: i64,
    edit: impl FnOnce(&mut UiTrackListRow) + Send + 'static,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let rows = ui.global::<Favorites>().get_tracks();
        if let Some(model) = TrackRows::installed(&rows, TRACKS_LABEL) {
            model.patch(id, edit);
        }
    });
}

/// Drop unfavourited tracks from the Songs tab and its selection on this tick, ahead of the
/// refresh the write's own `library_changed` tick brings.
pub fn remove_rows(weak: &Weak<AppWindow>, ids: Vec<i64>) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<Favorites>();
        let rows = g.get_tracks();
        if let Some(model) = TrackRows::installed(&rows, TRACKS_LABEL) {
            track_rows::remove_rows(&g, model, &ids);
        }
    });
}

/// The Songs tab's ids in display order, for `play-row` and the hero's Shuffle.
pub fn displayed_ids(g: &Favorites<'_>) -> Vec<i64> {
    TrackRows::ids_of(&g.get_tracks(), TRACKS_LABEL)
}

/// Empty `Favorites.tracks`, its ids and cached rows with it, for the section leave.
pub fn release_tracks(g: &Favorites<'_>) {
    let rows = g.get_tracks();
    if let Some(model) = TrackRows::installed(&rows, TRACKS_LABEL) {
        model.clear();
    }
}

fn current_query(fav_ui: &FavoritesUi) -> (TrackOrder, Needle) {
    let sort = current_sort(fav_ui);
    (TrackOrder::new(&sort.field, sort.dir), current_filter(fav_ui))
}

/// Show `keyset` in `Favorites.tracks` and run `then` over the model it went into, unless a newer
/// load was asked for since `ticket` or Songs is no longer what is mounted. UI thread only.
///
/// Both gates are asked here rather than trusted from the load, a leave or a tab pick being able
/// to land while it ran. A write refused for either re-arms the flag, so the pick that brings
/// Songs back knows nothing reached it.
fn show_tracks(
    ui: &AppWindow,
    fav_ui: &FavoritesUi,
    ticket: Ticket,
    keyset: Keyset<UiTrackListRow>,
    then: impl FnOnce(&Favorites<'_>, &TrackRows),
) {
    if !fav_ui.state().song_tickets.is_latest(ticket) {
        return;
    }
    if !fav_ui.section_active() || fav_ui.active_tab() != FavoritesTab::Songs {
        fav_ui.mark_songs_dirty();
        return;
    }
    let g = ui.global::<Favorites>();
    let rows = g.get_tracks();
    if let Some(model) = TrackRows::installed(&rows, TRACKS_LABEL) {
        model.show(keyset);
        then(&g, model);
    }
}
