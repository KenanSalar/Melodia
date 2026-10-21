//! The list half of a detail's fresh open, which every other load leaves to
//! [`track_rows`]' shared path: the open is the one load that runs under a query of its own rather
//! than the page's, and lands inside the closure that flips the detail's id.

use std::sync::Arc;

use super::{DetailGlobal, TrackDetail};
use crate::ui::list_selection::{self, RowSelectionView};
use crate::ui::paged_rows::track_rows::{self, ListQuery, TrackRows};
use crate::ui::paged_rows::{Keyset, Ticket};
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
    let loads = view.cache().loads();
    let ticket = loads.tickets.issue_reload();
    let query = ListQuery {
        scope: view.scope(id),
        order: TrackOrder::new(sort_field, SortDir::from_token(sort_dir)),
        needle: Needle::default(),
    };
    let keyset = track_rows::load(state, &query, &loads.scroll).await?;
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
        let tickets = &cache.loads().tickets;
        if !tickets.is_latest(self.ticket) {
            track_rows::requery(state, view);
            return;
        }
        let rows = g.track_rows();
        let Some(model) = TrackRows::installed(&rows, V::VIEW_ID) else { return };
        view.rows_moving(&g);
        // Every row it holds afterwards is one this open fetched, so none owes a re-read.
        model.clear();
        model.show(self.keyset);
        cache.record_shown(self.id, order, &Needle::default());
        tickets.reload_shown(self.ticket);
    }
}
