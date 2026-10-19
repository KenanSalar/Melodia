//! The loads every library card grid runs, over the [`CardGrid`] each one implements.
//!
//! A grid's ids and the cards near its viewport live in a [`PagedGrid`] installed once on its
//! global; what one grid differs in is its query, its store calls and what it states beside the
//! cards, which is the trait. A load reads the query and takes its ticket on the UI thread, so the
//! last one asked for is the one shown however they land, then runs the keyset and the seed on the
//! runtime. A **reload**, what a library change and a re-entry ask for, also re-reads every card
//! the grid holds and warms the seed's covers first; a **requery**, a sort or a keystroke, shows
//! the ids alone, unless it overtakes a reload still owed, which it then runs as.
//!
//! The cards convert on the UI thread as they land. Favorite Artists' subtitle is a translated
//! plural only a Slint callback resolves, and a card is a handful of strings.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, Weak};

use crate::ui::grid_prewarm::{self, GRID_PREWARM_AHEAD};
use crate::ui::paged_rows::grid::{GridRow, PagedGrid};
use crate::ui::paged_rows::{
    Keyset, KeysetTickets, Load, ReadStamp, RowRequest, ScrollPoint, spawn_fetch, window,
};
use melodia_app::state::AppState;
use melodia_core::entities::track_list::SortDir;
use melodia_core::error::{AppResult, describe};
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::AppWindow;

/// The card a grid of `K` draws.
pub type Card<K> = <<K as CardGrid>::Row as GridRow>::Card;

/// One library card grid.
pub trait CardGrid: Sized + 'static {
    /// The chunk struct the grid's `ListView` iterates.
    type Row: GridRow;
    /// A card as the store answers it.
    type Raw: Send + 'static;
    /// What a load runs under: the sort and the filter.
    type Query: Send + 'static;
    /// What the grid states beside its cards, answered with each load.
    type Facts: Send + 'static;
    /// The view handle the grid's loads live on.
    type Handle: Send + Sync + 'static;

    /// Names the grid in a log line.
    const LABEL: &'static str;

    fn loads(handle: &Self::Handle) -> &GridLoads;

    fn model(ui: &AppWindow) -> ModelRc<Self::Row>;

    fn columns(ui: &AppWindow) -> i32;

    fn query(ui: &AppWindow, handle: &Self::Handle) -> Self::Query;

    /// The grid's ids under `query`, and what it states beside them.
    fn keyset(
        state: AppState,
        query: Self::Query,
    ) -> impl Future<Output = AppResult<(Vec<i64>, Self::Facts)>> + Send;

    /// The cards behind `ids`, in the order asked.
    fn rows(
        state: AppState,
        ids: Vec<i64>,
    ) -> impl Future<Output = AppResult<Vec<Self::Raw>>> + Send;

    fn card(ui: &AppWindow, raw: &Self::Raw) -> (i64, Card<Self>);

    /// The cover a card draws, for the prewarm.
    fn artwork(raw: &Self::Raw) -> Option<&str>;

    /// Write `facts` beside a keyset of `shown` cards. UI thread.
    fn show_facts(ui: &AppWindow, handle: &Self::Handle, facts: Self::Facts, shown: usize);

    /// Whether the grid is what is drawn, so its covers are worth decoding.
    fn covers_on_screen(handle: &Self::Handle) -> bool;

    /// A load that stored nothing. A grid whose loads are gated on a dirty flag its caller
    /// consumed re-arms it here, so the next visit asks again.
    fn load_failed(_handle: &Self::Handle) {}
}

/// What one grid's loads share: the tickets that order them, where the grid was last read, and
/// the covers the last one opened on.
pub struct GridLoads {
    tickets: KeysetTickets,
    scroll: ScrollPoint,
    covers: Mutex<Vec<PathBuf>>,
}

impl GridLoads {
    pub fn new(scroll: ScrollPoint) -> Self {
        Self { tickets: KeysetTickets::default(), scroll, covers: Mutex::default() }
    }
}

/// A grid's sort field, direction and needle, as its global spells them.
pub struct SortedQuery<S> {
    pub sort: S,
    pub dir: SortDir,
    pub needle: Needle,
}

impl<S> SortedQuery<S> {
    pub fn new(sort: S, dir_token: &str, filter: &str) -> Self {
        Self { sort, dir: SortDir::from_token(dir_token), needle: fold_needle(filter) }
    }
}

/// The count a header states whatever the filter leaves: `ids`' own length unfiltered, else
/// `count`'s answer.
pub async fn unfiltered_total(
    ids: &[i64],
    needle: &Needle,
    count: impl Future<Output = AppResult<i64>>,
) -> AppResult<i32> {
    let total = if needle.is_empty() {
        i64::try_from(ids.len()).unwrap_or(i64::MAX)
    } else {
        count.await?
    };
    Ok(i32::try_from(total).unwrap_or(i32::MAX))
}

/// An empty grid of `K`, to install once on its global.
pub fn install<K: CardGrid>(state: &AppState, ui: &AppWindow) -> Rc<PagedGrid<K::Row>> {
    let grid = PagedGrid::new(fetcher::<K>(state, ui.as_weak()));
    grid.set_columns(K::columns(ui));
    grid
}

/// Fetches the cards a grid of `K` asks for.
fn fetcher<K: CardGrid>(
    state: &AppState,
    weak: Weak<AppWindow>,
) -> impl Fn(RowRequest<Card<K>>) + 'static {
    let state = state.clone();
    move |request| {
        let rows = K::rows(state.clone(), request.ids().to_vec());
        let weak = weak.clone();
        let land = move |request: RowRequest<Card<K>>, raw: Vec<K::Raw>| {
            // Dropped, the request frees the grid to ask again once there is a window.
            let Some(ui) = weak.upgrade() else { return };
            request.land(raw.iter().map(|raw| K::card(&ui, raw)).collect());
        };
        spawn_fetch(&state.runtime, request, rows, land, K::LABEL);
    }
}

/// Which cards a load reads back beside its ids.
enum Seed {
    /// The window around where the grid is scrolled, which it opens on.
    Window,
    /// These alone, the only ones the change behind the load can have moved.
    Only(Vec<i64>),
}

/// Reload `K`: its ids, the cards it opens on with their covers warm, and every card it holds
/// read again. UI thread.
pub fn reload<K: CardGrid>(ui: &AppWindow, state: &AppState, handle: &Arc<K::Handle>) {
    let ticket = K::loads(handle).tickets.issue_reload();
    spawn_load::<K>(ui, state, handle, Load::Reload(ticket), Seed::Window);
}

/// Load `K`'s ids under a new sort or filter, the cards it holds being current. UI thread.
pub fn requery<K: CardGrid>(ui: &AppWindow, state: &AppState, handle: &Arc<K::Handle>) {
    let load = K::loads(handle).tickets.issue_requery();
    spawn_load::<K>(ui, state, handle, load, Seed::Window);
}

/// Load `K`'s ids again with only `ids` read back, for a change that can have moved those cards
/// and no other. UI thread.
pub fn refresh_cards<K: CardGrid>(
    ui: &AppWindow,
    state: &AppState,
    handle: &Arc<K::Handle>,
    ids: Vec<i64>,
) {
    let load = K::loads(handle).tickets.issue_requery();
    spawn_load::<K>(ui, state, handle, load, Seed::Only(ids));
}

/// Chunk `K` at `columns` cards a row, keeping every cached card. UI thread.
pub fn set_columns<K: CardGrid>(ui: &AppWindow, columns: i32) {
    if let Some(grid) = PagedGrid::installed(&K::model(ui), K::LABEL) {
        grid.set_columns(columns);
    }
}

/// Empty `K` for a section leave, refusing every load still on its way: nothing it brings has a
/// grid on screen to go to, and the next enter reloads. UI thread.
pub fn release<K: CardGrid>(ui: &AppWindow, handle: &K::Handle) {
    if let Some(grid) = PagedGrid::installed(&K::model(ui), K::LABEL) {
        grid.clear();
    }
    K::loads(handle).tickets.revoke();
}

/// Warm the covers `K`'s last load opened on, if its grid is what is drawn: for a grid coming back
/// under a detail that closed, or a restore that failed. Blocking.
pub fn prewarm_seeded<K: CardGrid>(handle: &K::Handle) {
    if !K::covers_on_screen(handle) {
        return;
    }
    let paths = K::loads(handle).covers.lock().clone();
    if !paths.is_empty() {
        grid_prewarm::prewarm(&paths);
    }
}

fn spawn_load<K: CardGrid>(
    ui: &AppWindow,
    state: &AppState,
    handle: &Arc<K::Handle>,
    load: Load,
    seed: Seed,
) {
    let query = K::query(ui, handle);
    let state = state.clone();
    let handle = Arc::clone(handle);
    let weak = ui.as_weak();
    state.runtime.clone().spawn(async move {
        if let Err(e) = load_now::<K>(state, Arc::clone(&handle), weak, load, seed, query).await {
            log::warn!("{}: load failed: {}", K::LABEL, describe(&e));
            K::load_failed(&handle);
        }
    });
}

async fn load_now<K: CardGrid>(
    state: AppState,
    handle: Arc<K::Handle>,
    weak: Weak<AppWindow>,
    load: Load,
    seed: Seed,
    query: K::Query,
) -> AppResult<()> {
    let (Load::Requery(ticket) | Load::Reload(ticket)) = load;
    let reload = matches!(load, Load::Reload(_));
    let read = ReadStamp::now();
    let (ids, facts) = K::keyset(state.clone(), query).await?;
    let loads = K::loads(&handle);
    let opens_on_window = matches!(seed, Seed::Window);
    let seed_ids = match seed {
        // Asked once the ids are in, the latest the scroll position can be read before the seed.
        Seed::Window => ids[window(ids.len(), loads.scroll.row())].to_vec(),
        Seed::Only(ids) => ids,
    };
    let seed = K::rows(state, seed_ids).await?;
    if !loads.tickets.is_latest(ticket) {
        return Ok(());
    }
    if opens_on_window {
        let covers =
            grid_prewarm::unique_artwork_paths(seed.iter().map(K::artwork), GRID_PREWARM_AHEAD);
        loads.covers.lock().clone_from(&covers);
        if reload
            && K::covers_on_screen(&handle)
            && let Err(e) =
                tokio::task::spawn_blocking(move || grid_prewarm::prewarm(&covers)).await
        {
            log::warn!("{}: cover prewarm failed: {}", K::LABEL, describe(&e));
        }
    }

    let _ = weak.upgrade_in_event_loop(move |ui| {
        let loads = K::loads(&handle);
        if !loads.tickets.is_latest(ticket) {
            return;
        }
        let model = K::model(&ui);
        let Some(grid) = PagedGrid::installed(&model, K::LABEL) else {
            return;
        };
        let shown = ids.len();
        let seed = seed.iter().map(|raw| K::card(&ui, raw)).collect();
        // A grid released while hidden missed every column change since.
        grid.set_columns(K::columns(&ui));
        grid.show(Keyset { ids: ids.into(), seed, read });
        if reload {
            grid.reread();
            loads.tickets.reload_shown(ticket);
        }
        K::show_facts(&ui, &handle, facts, shown);
    });
    Ok(())
}
