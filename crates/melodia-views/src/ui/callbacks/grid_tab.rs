//! The section lifecycle the four My Library grid tabs (Albums, Artists, Genres and Playlists)
//! share, wired once over [`GridTab`]: a leave hands back the grid and the detail, an enter
//! reloads what the leave released and reopens the detail it emptied, and a library change
//! reloads the tab on screen, its grid on the detail's close where one covers it. Each tab's
//! `callbacks/lifecycle.rs` states what it differs in.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::macros::spawn_logged;
use crate::ui::card_grid::{self, CardGrid};
use crate::ui::grid_prewarm;
use crate::ui::list_selection;
use crate::ui::my_library::tab_is_mounted;
use crate::ui::signal::on_signal;
use crate::ui::track_detail::{self, DetailGlobal, TrackDetail, restore};
use melodia_app::state::AppState;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, CardSelection, NavEnterFrom};

/// A My Library tab whose card grid drills into a track detail, as the shared lifecycle drives it.
pub(in crate::ui) trait GridTab: TrackDetail {
    /// The tab's card grid, whose loads live on this handle.
    type Grid: CardGrid<Handle = Self>;

    fn bind_section_active_changed(ui: &AppWindow, f: impl FnMut(bool) + 'static);

    /// Puts the grid's `total-count` back to the unfetched sentinel.
    fn rewind_count(ui: &AppWindow);

    /// Hands the detail's hero back through the shared teardown, on a leave and on a reopen that
    /// failed.
    fn release_hero(ui: &AppWindow, g: &Self::Global);

    /// Empties what the detail lists beside its tracks, on a leave. Artist Detail's Albums strip
    /// is the one.
    fn release_beside(&self, _g: &Self::Global) {}

    /// Drops what the tab keeps resident beyond its grid and the detail's rows. Blocking, and only
    /// while the tab is still off screen; the leave trims after it.
    fn release_section_state(&self) {}

    fn open(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
        enter_from: NavEnterFrom,
    ) -> impl Future<Output = AppResult<()>> + Send;

    /// Re-reads an open detail after a library change, keeping its sort, filter and selection.
    fn refresh(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
    ) -> impl Future<Output = AppResult<()>> + Send;

    /// Reopens the detail the launch parked for this tab's first enter.
    fn restore(ui: &AppWindow, state: &AppState, view: &Arc<Self>, id: i64);
}

/// Wires `view`'s section gate and its `library_changed` subscriber. The shadow is seeded from
/// the mounted tab, the gate firing only on a later change (`ui-patterns.md`'s
/// `SectionActiveGate` entry).
pub(in crate::ui) fn wire<V: GridTab>(ui: &AppWindow, state: &AppState, view: &Arc<V>) {
    view.section().set_active(tab_is_mounted(ui, V::TAB));
    wire_section_gate(ui, state, view);
    install_library_refresher(ui, state, view);
}

fn wire_section_gate<V: GridTab>(ui: &AppWindow, state: &AppState, view: &Arc<V>) {
    let view = Arc::clone(view);
    let s = state.clone();
    let weak = ui.as_weak();
    V::bind_section_active_changed(ui, move |active| {
        view.section().set_active(active);
        if active {
            restore::resume_deferred(&weak, &s, &view, V::restore);
            if let Some(ui) = weak.upgrade() {
                reenter(&ui, &s, &view);
            }
            return;
        }
        // Before the release spawns, so the next enter's `take_dirty()` can't find a grid the
        // release hasn't reached yet.
        view.section().mark_dirty();
        if let Some(ui) = weak.upgrade() {
            release_on_ui_thread(&ui, &*view);
            // The card grid's set goes back with the models it describes; `card-selection.slint`
            // argues why it cannot outlive the leave.
            ui.global::<CardSelection>().invoke_clear();
        }
        let view = Arc::clone(&view);
        s.runtime.spawn_blocking(move || release_off_thread(&*view));
    });
}

/// The leave's UI-thread half. The grid and the detail hold `SharedString`s and image refs that
/// drop only when handed back here. The detail id stays, for the next enter to reopen.
fn release_on_ui_thread<V: GridTab>(ui: &AppWindow, view: &V) {
    // Rewound on the same tick as the grid it numbers; `Albums.total-count`'s declaration argues
    // the sentinel.
    V::rewind_count(ui);
    card_grid::release::<V::Grid>(ui, view);

    let Some(g) = view.cache().global().upgrade() else { return };
    V::release_hero(ui, &g);
    track_detail::release_rows(view, &g);
    view.release_beside(&g);
    list_selection::clear_row_selection(&g);
}

/// The leave's blocking half, skipped where the tab is back before the pool reached it: the enter
/// is already reloading into what this would hand back.
fn release_off_thread<V: GridTab>(view: &V) {
    if view.section().active() {
        return;
    }
    view.release_section_state();
    melodia_platform::services::platform::allocator::trim();
}

/// The tab came back on screen: a reload where a leave released the grid, else a prewarm of its
/// covers (the first enter after the launch's load). The detail the release emptied is reopened
/// beside the reload, so the user lands back where they were.
fn reenter<V: GridTab>(ui: &AppWindow, s: &AppState, view: &Arc<V>) {
    if !view.section().take_dirty() {
        let view = Arc::clone(view);
        s.runtime.spawn_blocking(move || card_grid::prewarm_seeded::<V::Grid>(&view));
        return;
    }
    card_grid::reload::<V::Grid>(ui, s, view);
    let open_id = view.cache().id();
    if open_id >= 0 {
        s.runtime.spawn(reopen(s.clone(), Arc::clone(view), ui.as_weak(), open_id));
    }
}

/// Reopens the detail a leave emptied. A failure most likely means the entity went while the tab
/// was hidden, the refresher being gated on the section, so the user is handed the grid rather
/// than an empty detail only the back button leaves.
async fn reopen<V: GridTab>(s: AppState, view: Arc<V>, weak: Weak<AppWindow>, id: i64) {
    let reopened = V::open(&s, &view, weak.clone(), id, NavEnterFrom::Right).await;
    let Err(e) = reopened else { return };
    log::warn!("{}::section_enter reopen({id}): {}", V::VIEW_ID, describe(&e));
    view.clear_detail();
    // Handed back warm, as a failed restore's grid is.
    grid_prewarm::prewarm_off_thread(&view, card_grid::prewarm_seeded::<V::Grid>).await;
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let Some(g) = view.cache().global().upgrade() else { return };
        g.set_detail_id(-1);
        V::release_hero(&ui, &g);
        // A change landing during the reopen saw the id and owed the grid its reload.
        reload_owed_grid(&ui, &s, &view);
    });
}

/// Watcher ticks, scans, folder changes and every star or heart bump `library_changed`: an open
/// detail refreshes, which keeps the user's sort and selection where a reopen would reset them,
/// and the grid reloads, or is owed a reload while a detail covers it ([`reload_owed_grid`]).
fn install_library_refresher<V: GridTab>(ui: &AppWindow, state: &AppState, view: &Arc<V>) {
    let s = state.clone();
    let view = Arc::clone(view);
    let installed = on_signal(&state.library_changed, ui.as_weak(), V::Grid::LABEL, move |ui| {
        // A hidden tab has had (or is about to have) everything a reload repopulates released.
        // Marked rather than dropped, or a change landing before the tab's first visit (the first
        // scan on a fresh database) leaves that enter only a prewarm.
        if !view.section().active() {
            view.section().mark_dirty();
            return;
        }
        let open_id = view.cache().id();
        if open_id < 0 {
            card_grid::reload::<V::Grid>(ui, &s, &view);
            return;
        }
        view.section().owe_grid();
        let s = s.clone();
        let view = Arc::clone(&view);
        let weak = ui.as_weak();
        spawn_logged!(
            s,
            format_args!("{}::library_changed refresh", V::VIEW_ID),
            V::refresh(&s, &view, weak, open_id)
        );
    });
    if let Err(e) = installed {
        log::warn!("{} won't follow library changes: {}", V::Grid::LABEL, describe(&e));
    }
}

/// Reloads the grid a detail hands back to on the same page, where a library change under the
/// detail owed it one, returning whether it did. The reload warms the covers it opens on, so a
/// close that gets `true` skips its own prewarm. Call it once the detail's id is cleared, which
/// the cover check reads.
pub(in crate::ui) fn reload_owed_grid<V: GridTab>(
    ui: &AppWindow,
    state: &AppState,
    view: &Arc<V>,
) -> bool {
    if !view.section().take_grid_owed() {
        return false;
    }
    card_grid::reload::<V::Grid>(ui, state, view);
    true
}
