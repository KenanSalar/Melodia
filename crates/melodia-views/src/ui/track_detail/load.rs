//! The list half of every detail's open, reload and requery, over the [`TrackRows`] its global
//! holds.
//!
//! A load takes its ticket before reading the sort and filter it runs under, so whichever was asked
//! for last is the one shown however they land, and a sort or keystroke asked for while a library
//! change's reload is still owed runs as that reload. It shows only while its detail still lists
//! the entity it loaded, and a close or a leave revokes every ticket out, so no load already on its
//! way lands in a model they released. An open landing after one asks again, being a navigation of
//! its own.

use std::sync::Arc;

use super::{DetailGlobal, TrackDetail};
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::list_selection::{self, RowSelectionView};
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::paged_rows::{Keyset, Load, Ticket};
use melodia_app::state::AppState;
use melodia_core::entities::track_list::{SortDir, TrackOrder};
use melodia_core::error::AppResult;
use melodia_core::utils::needle::Needle;
use melodia_ui::TrackListRow as UiTrackListRow;

/// The tracks a fresh open of one entity shows, fetched before the hop that flips the detail's id.
pub struct Opening {
    id: i64,
    sort_field: String,
    sort_dir: String,
    keyset: Keyset<UiTrackListRow>,
    ticket: Ticket,
}

/// Fetch the ids `view` lists for `id` under the persisted sort, unfiltered, with the rows the list
/// opens on and their covers warm.
pub async fn open<V: TrackDetail>(
    state: &AppState,
    view: &V,
    id: i64,
    sort_field: &str,
    sort_dir: &str,
) -> AppResult<Opening> {
    let cache = view.cache();
    let ticket = cache.tickets.issue_reload();
    let order = TrackOrder::new(sort_field, SortDir::from_token(sort_dir));
    let keyset =
        track_rows::load(state, V::scope(id), order, &Needle::default(), &cache.scroll).await?;
    track_rows::prewarm(view.row_covers(), &keyset).await;
    Ok(Opening {
        id,
        sort_field: sort_field.to_owned(),
        sort_dir: sort_dir.to_owned(),
        keyset,
        ticket,
    })
}

impl Opening {
    /// Seat the open on its page: its sort, no filter, no selection and its rows. UI thread, inside
    /// the closure that flips the detail's id, so the page never paints the previous entity's list.
    ///
    /// A load asked for on the page being left can overtake it, a keystroke there reading that
    /// page's entity, so an overtaken open asks again under its own query. Where a library change
    /// overtook it, that requery runs as the reload it owes.
    pub fn seat<V: TrackDetail>(self, state: &AppState, view: &Arc<V>) {
        let cache = view.cache();
        let Some(g) = cache.global().upgrade() else { return };
        let order = TrackOrder::new(&self.sort_field, SortDir::from_token(&self.sort_dir));
        cache.seat(self.id, order);
        g.set_sort(&self.sort_field, &self.sort_dir);
        g.clear_filter();
        list_selection::clear_row_selection(&g);
        if !cache.tickets.is_latest(self.ticket) {
            requery(state, view);
            return;
        }
        let rows = g.track_rows();
        let Some(model) = TrackRows::installed(&rows, V::VIEW_ID) else { return };
        view.rows_moving(&g);
        // Every row it holds afterwards is one this open fetched, so none owes a re-read.
        model.clear();
        model.show(self.keyset);
        cache.record_shown(self.id, order, &Needle::default());
        cache.tickets.reload_shown(self.ticket);
    }
}

/// Reload the open detail: its ids under the sort and filter the page shows, the rows it opens on
/// warm, and every row it still holds read again. What a library change, a playlist edit and a
/// play-count flush ask for.
pub async fn reload<V: TrackDetail>(state: &AppState, view: &Arc<V>) -> AppResult<()> {
    let ticket = view.cache().tickets.issue_reload();
    reload_under(state, view, ticket).await
}

/// Reload the ids after a header click or a keystroke. The rows the list holds are still current,
/// so it re-reads none and warms no covers, unless it overtakes a reload, which it then runs in
/// that one's place.
pub fn requery<V: TrackDetail>(state: &AppState, view: &Arc<V>) {
    let load = view.cache().tickets.issue_requery();
    let s = state.clone();
    let view = Arc::clone(view);
    match load {
        Load::Requery(ticket) => {
            spawn_logged!(
                s,
                format_args!("{}::requery", V::VIEW_ID),
                requery_now(&s, view, ticket)
            );
        }
        Load::Reload(ticket) => {
            spawn_logged!(
                s,
                format_args!("{}::reload", V::VIEW_ID),
                reload_under(&s, &view, ticket)
            );
        }
    }
}

async fn reload_under<V: TrackDetail>(
    state: &AppState,
    view: &Arc<V>,
    ticket: Ticket,
) -> AppResult<()> {
    let cache = view.cache();
    let (id, order, needle) = cache.query();
    if id < 0 {
        return Ok(());
    }
    let keyset = track_rows::load(state, V::scope(id), order, &needle, &cache.scroll).await?;
    // Overtaken already, so nothing this warms would be drawn.
    if !cache.tickets.is_latest(ticket) {
        return Ok(());
    }
    track_rows::prewarm(view.row_covers(), &keyset).await;

    let unfiltered = needle.is_empty();
    let global = cache.global().clone();
    let view = Arc::clone(view);
    let _ = global.upgrade_in_event_loop(move |g| {
        show(&*view, &g, (id, order, needle), ticket, keyset, |model| {
            track_rows::settle_reload(&g, model, &view.cache().tickets, ticket, unfiltered);
        });
    });
    Ok(())
}

async fn requery_now<V: TrackDetail>(
    state: &AppState,
    view: Arc<V>,
    ticket: Ticket,
) -> AppResult<()> {
    let cache = view.cache();
    let (id, order, needle) = cache.query();
    if id < 0 {
        return Ok(());
    }
    let keyset = track_rows::load(state, V::scope(id), order, &needle, &cache.scroll).await?;
    let global = cache.global().clone();
    let _ = global.upgrade_in_event_loop(move |g| {
        show(&*view, &g, (id, order, needle), ticket, keyset, |_| {});
    });
    Ok(())
}

/// Show `keyset`, loaded for entity `id` under `order` and `needle`, and run `then` over the model
/// it went into, unless a newer load was asked for since `ticket` or the detail no longer lists
/// `id`. UI thread.
fn show<V: TrackDetail>(
    view: &V,
    g: &V::Global,
    (id, order, needle): (i64, TrackOrder, Needle),
    ticket: Ticket,
    keyset: Keyset<UiTrackListRow>,
    then: impl FnOnce(&TrackRows),
) {
    let cache = view.cache();
    if !cache.tickets.is_latest(ticket) || cache.id() != id {
        return;
    }
    let rows = g.track_rows();
    let Some(model) = TrackRows::installed(&rows, V::VIEW_ID) else { return };
    if *model.ids() != *keyset.ids {
        view.rows_moving(g);
    }
    model.show(keyset);
    cache.record_shown(id, order, &needle);
    then(model);
}
