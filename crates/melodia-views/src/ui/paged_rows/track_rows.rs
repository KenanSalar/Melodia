//! [`PagedRows`] over track-list rows, the binding the Songs tabs and the four details share: the
//! placeholder, the row fetch, the keyset load that opens a list on the screenful it is scrolled
//! to, and what a reload owes once it shows.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use super::{
    Keyset, KeysetTickets, PagedRow, PagedRows, ReadStamp, RowRequest, ScrollPoint, Ticket,
    spawn_fetch, window,
};
use crate::ui::grid_prewarm;
use crate::ui::list_selection::{self, RowSelectionView};
use crate::ui::tracks::to_slint_track_list_row;
use crate::ui::util::{StringPool, clamp_i64_to_i32};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::entities::track_list::{TrackListScope, TrackOrder};
use melodia_core::error::{AppResult, describe};
use melodia_core::utils::needle::Needle;
use melodia_ui::TrackListRow as UiTrackListRow;

pub type TrackRows = PagedRows<UiTrackListRow>;

impl PagedRow for UiTrackListRow {
    // Blank but live: a row clicked before its fetch lands still plays, queues and picks by id.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), enabled: true, ..Self::default() }
    }

    fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
    }
}

/// Fetches the rows a track list asks for.
pub fn fetcher(state: &AppState) -> impl Fn(RowRequest<UiTrackListRow>) + 'static {
    fetch_by_id(state, convert)
}

/// Fetches the rows a list asks for through the track-list projection, converted by `convert` on
/// the runtime beside the query ([`spawn_fetch`]).
pub fn fetch_by_id<R: PagedRow + Send>(
    state: &AppState,
    convert: fn(&[RsTrackListRow]) -> Vec<(i64, R)>,
) -> impl Fn(RowRequest<R>) + 'static {
    let db = state.db.clone();
    let runtime = state.runtime.clone();
    move |request| {
        let db = db.clone();
        let ids = request.ids().to_vec();
        let rows =
            async move { library::keyset::rows_by_ids(&db, &ids).await.map(|rows| convert(&rows)) };
        spawn_fetch(&runtime, request, rows, RowRequest::land, "track rows");
    }
}

/// The ids in `scope` that `needle` matches, in `order`, with the rows of the [`window`] around
/// `at`, so the list shows where it is scrolled rather than on placeholders.
pub async fn load(
    state: &AppState,
    scope: TrackListScope,
    order: TrackOrder,
    needle: &Needle,
    at: &ScrollPoint,
) -> AppResult<Keyset<UiTrackListRow>> {
    let read = ReadStamp::now();
    let ids = library::keyset::track_ids(&state.db, scope, order, needle).await?;
    // Asked once the ids are in, the latest the scroll position can be read before the seed.
    let seed_ids = &ids[window(ids.len(), at.row())];
    let seed = convert(&library::keyset::rows_by_ids(&state.db, seed_ids).await?);
    Ok(Keyset { ids: ids.into(), seed, read })
}

/// Warm the row covers `keyset` opens on, in display order, before it shows: a cold tier would
/// paint the first screen on placeholders. Every row past the seed asks the tier itself as it
/// scrolls into view.
pub async fn prewarm(thumbs: &Arc<CoverThumbs>, keyset: &Keyset<UiTrackListRow>) {
    let paths: Vec<PathBuf> = grid_prewarm::unique_artwork_paths(
        keyset.seed.iter().map(|(_, row)| Some(row.artwork_path.as_str())),
        thumbs.capacity(),
    );
    if paths.is_empty() {
        return;
    }
    let thumbs = Arc::clone(thumbs);
    if let Err(e) = tokio::task::spawn_blocking(move || thumbs.prewarm(&paths)).await {
        log::warn!("track rows: cover prewarm failed: {}", describe(&e));
    }
}

/// What a reload owes once its keyset shows: every row the list still holds read again, the reload
/// settled, and the picks the list no longer holds dropped. Only an unfiltered keyset can say which
/// those are, a filter hiding rows without dropping their picks. UI thread.
pub fn settle_reload(
    view: &impl RowSelectionView,
    model: &TrackRows,
    tickets: &KeysetTickets,
    ticket: Ticket,
    unfiltered: bool,
) {
    model.reread();
    tickets.reload_shown(ticket);
    if unfiltered {
        list_selection::keep_selection_within(view, || model.row_ids().into_iter().collect());
    }
}

/// Drop `ids` from the list and from its selection, for rows the user's own write just took out,
/// ahead of the reload it brings. That reload can't drop them under a filter (`settle_reload`), and
/// a pick left behind is one the next batch action acts on unseen. UI thread.
pub fn remove_rows(view: &impl RowSelectionView, model: &TrackRows, ids: &[i64]) {
    model.remove(&ids.iter().copied().collect::<HashSet<i64>>());
    let removed: HashSet<i32> = ids.iter().map(|&id| clamp_i64_to_i32(id)).collect();
    list_selection::forget_rows(view, &removed);
}

/// One fetch's rows, sharing a pool so an album's tracks share their strings.
fn convert(rows: &[RsTrackListRow]) -> Vec<(i64, UiTrackListRow)> {
    let mut pool = StringPool::default();
    rows.iter().map(|row| (row.id, to_slint_track_list_row(row, &mut pool))).collect()
}
