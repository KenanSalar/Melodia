//! [`PagedRows`] over track-list rows, the binding the Songs tabs and the four details share: the
//! placeholder, the row fetch, and the one load path every such list runs, over the
//! [`PagedTrackList`] each implements. The grids' twin is `ui::card_grid`.
//!
//! A load takes its ticket before it reads the query it runs under, so whichever was asked for
//! last is the one shown however they land. A **reload**, what a library change, a re-enter and a
//! play-count flush ask for, also warms the covers of the rows it opens on and reads every row the
//! list holds again; a **requery**, a sort or a keystroke, shows the ids alone, unless it overtakes
//! a reload still owed, which it then runs as.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{StrongHandle, Weak};

use super::{
    Keyset, KeysetTickets, Load, PagedRow, PagedRows, ReadStamp, RowRequest, ScrollPoint, Ticket,
    spawn_fetch, window,
};
use crate::ui::callbacks::macros::spawn_logged;
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

/// One list over a [`TrackRows`]: the two Songs tabs, and the four details through
/// `track_detail`'s blanket impl. What one differs in is where its query comes from, when a load
/// may still land, and what it writes beside the rows.
pub trait PagedTrackList: Send + Sync + 'static {
    /// The global the rows live on, which a load lands through.
    type Global: RowSelectionView + StrongHandle + 'static;
    /// What a reload writes beside the rows, answered with its keyset.
    type Facts: Default + Send + 'static;

    /// Names the list in a log line.
    const LABEL: &'static str;

    fn loads(&self) -> &ListLoads;

    fn global(&self) -> &Weak<Self::Global>;

    /// The row tier a reload warms the rows it opens on into.
    fn row_covers(&self) -> &Arc<CoverThumbs>;

    /// What a load runs under, read once it holds its ticket. `None` where nothing is listed.
    fn query(&self) -> Option<ListQuery>;

    /// What a reload under `query` writes beside the `ids` it found, the default for a list that
    /// writes nothing there.
    fn facts(
        _state: &AppState,
        _query: &ListQuery,
        _ids: &[i64],
    ) -> impl Future<Output = AppResult<Self::Facts>> + Send {
        std::future::ready(Ok(Self::Facts::default()))
    }

    /// Whether a load under `query` may still show. A reload asks once its ids are in and again
    /// after its covers are warm, and every load asks on the UI thread before the rows go in.
    fn still_wanted(&self, _query: &ListQuery) -> bool {
        true
    }

    /// A load that will show nothing, having failed or been refused by [`Self::still_wanted`]. A
    /// list whose loads are gated on a dirty flag its caller consumed re-arms it here.
    fn load_dropped(&self) {}

    /// The ids about to show differ from the ones on screen. UI thread.
    fn rows_moving(&self) {}

    /// A keyset loaded under `query` just went in, with what it wrote beside it where it was a
    /// reload. UI thread.
    fn shown(&self, _g: &Self::Global, _query: &ListQuery, _facts: Option<Self::Facts>) {}
}

/// What one list's loads share: the tickets that order them, and where the list was last read.
pub struct ListLoads {
    pub tickets: KeysetTickets,
    pub scroll: ScrollPoint,
}

impl ListLoads {
    pub fn new(scroll: ScrollPoint) -> Self {
        Self { tickets: KeysetTickets::default(), scroll }
    }
}

/// What a load runs under: which tracks, in what order, narrowed by what.
pub struct ListQuery {
    pub scope: TrackListScope,
    pub order: TrackOrder,
    pub needle: Needle,
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

/// The ids `query` lists, with the rows of the [`window`] around `at`, so the list shows where it
/// is scrolled rather than on placeholders.
pub async fn load(
    state: &AppState,
    query: &ListQuery,
    at: &ScrollPoint,
) -> AppResult<Keyset<UiTrackListRow>> {
    let read = ReadStamp::now();
    let ids =
        library::keyset::track_ids(&state.db, query.scope, query.order, &query.needle).await?;
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

/// Reload `list`: its ids under the query it shows, the rows it opens on with their covers warm,
/// and every row it still holds read again.
pub async fn reload<L: PagedTrackList>(state: &AppState, list: &Arc<L>) -> AppResult<()> {
    let ticket = list.loads().tickets.issue_reload();
    reload_under(state, list, ticket).await
}

/// Load `list`'s ids after a header click or a keystroke. The rows it holds are still current, so
/// it reads none of them again and warms no covers, unless it overtakes a reload, which it then
/// runs in that one's place.
pub fn requery<L: PagedTrackList>(state: &AppState, list: &Arc<L>) {
    let load = list.loads().tickets.issue_requery();
    let s = state.clone();
    let list = Arc::clone(list);
    match load {
        Load::Requery(ticket) => {
            spawn_logged!(s, format_args!("{}::requery", L::LABEL), requery_now(&s, list, ticket));
        }
        Load::Reload(ticket) => {
            spawn_logged!(s, format_args!("{}::reload", L::LABEL), reload_under(&s, &list, ticket));
        }
    }
}

/// Patch the rows showing `ids` after a heart or a star, a no-op for an id the list doesn't hold.
/// Neither moves a track in or out of a list, so nothing is fetched and the scroll holds. Safe
/// from any thread.
pub fn patch_rows<L: PagedTrackList>(
    list: &L,
    ids: Vec<i64>,
    edit: impl Fn(&mut UiTrackListRow) + Send + 'static,
) {
    let _ = list.global().upgrade_in_event_loop(move |g| {
        let rows = g.track_rows();
        if let Some(model) = TrackRows::installed(&rows, L::LABEL) {
            for id in ids {
                model.patch(id, &edit);
            }
        }
    });
}

/// Drop `ids` from the list and from its selection ([`remove_rows_from`]). Safe from any thread.
pub fn remove_rows<L: PagedTrackList>(list: &L, ids: Vec<i64>) {
    let _ = list.global().upgrade_in_event_loop(move |g| {
        let rows = g.track_rows();
        if let Some(model) = TrackRows::installed(&rows, L::LABEL) {
            remove_rows_from(&g, model, &ids);
        }
    });
}

/// Drop `ids` from `model` and from `view`'s selection, for rows the user's own write just took
/// out, ahead of the reload it brings. That reload can't drop them under a filter, and a pick left
/// behind is one the next batch action acts on unseen. UI thread.
pub fn remove_rows_from(view: &impl RowSelectionView, model: &TrackRows, ids: &[i64]) {
    model.remove(&ids.iter().copied().collect::<HashSet<i64>>());
    let removed: HashSet<i32> = ids.iter().map(|&id| clamp_i64_to_i32(id)).collect();
    list_selection::forget_rows(view, &removed);
}

async fn reload_under<L: PagedTrackList>(
    state: &AppState,
    list: &Arc<L>,
    ticket: Ticket,
) -> AppResult<()> {
    let Some(query) = list.query() else {
        return Ok(());
    };
    let keyset =
        load(state, &query, &list.loads().scroll).await.inspect_err(|_| list.load_dropped())?;
    let facts = L::facts(state, &query, &keyset.ids).await.inspect_err(|_| list.load_dropped())?;
    if !list.still_wanted(&query) {
        list.load_dropped();
        return Ok(());
    }
    // Overtaken already, so nothing this warms would be drawn.
    if !list.loads().tickets.is_latest(ticket) {
        return Ok(());
    }
    prewarm(list.row_covers(), &keyset).await;
    // Asked again after the slow part, before which a leave may not have happened yet.
    if !list.still_wanted(&query) {
        list.load_dropped();
        return Ok(());
    }
    land(
        Arc::clone(list),
        Landing { query, keyset, load: Load::Reload(ticket), facts: Some(facts) },
    );
    Ok(())
}

async fn requery_now<L: PagedTrackList>(
    state: &AppState,
    list: Arc<L>,
    ticket: Ticket,
) -> AppResult<()> {
    let Some(query) = list.query() else {
        return Ok(());
    };
    let keyset =
        load(state, &query, &list.loads().scroll).await.inspect_err(|_| list.load_dropped())?;
    land(list, Landing { query, keyset, load: Load::Requery(ticket), facts: None });
    Ok(())
}

/// A keyset on its way to the UI thread, with what it was loaded under.
struct Landing<F> {
    query: ListQuery,
    keyset: Keyset<UiTrackListRow>,
    load: Load,
    facts: Option<F>,
}

fn land<L: PagedTrackList>(list: Arc<L>, landing: Landing<L::Facts>) {
    let global = list.global().clone();
    let _ = global.upgrade_in_event_loop(move |g| show(&*list, &g, landing));
}

/// Show `landing`, unless a newer load was asked for since or the list no longer wants it, and
/// settle a reload once it has: every row the list still holds read again, and the picks it no
/// longer holds dropped. UI thread.
fn show<L: PagedTrackList>(list: &L, g: &L::Global, landing: Landing<L::Facts>) {
    let Landing { query, keyset, load, facts } = landing;
    let (Load::Requery(ticket) | Load::Reload(ticket)) = load;
    let tickets = &list.loads().tickets;
    if !tickets.is_latest(ticket) {
        return;
    }
    if !list.still_wanted(&query) {
        list.load_dropped();
        return;
    }
    let rows = g.track_rows();
    let Some(model) = TrackRows::installed(&rows, L::LABEL) else {
        return;
    };
    if *model.ids() != *keyset.ids {
        list.rows_moving();
    }
    model.show(keyset);
    list.shown(g, &query, facts);
    if matches!(load, Load::Requery(_)) {
        return;
    }
    model.reread();
    tickets.reload_shown(ticket);
    // Only an unfiltered keyset can say which picks the list no longer holds, a filter hiding
    // rows without dropping their picks.
    if query.needle.is_empty() {
        list_selection::keep_selection_within(g, || model.row_ids().into_iter().collect());
    }
}

/// One fetch's rows, sharing a pool so an album's tracks share their strings.
fn convert(rows: &[RsTrackListRow]) -> Vec<(i64, UiTrackListRow)> {
    let mut pool = StringPool::default();
    rows.iter().map(|row| (row.id, to_slint_track_list_row(row, &mut pool))).collect()
}
