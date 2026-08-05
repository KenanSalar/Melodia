//! Browser-style in-memory back/forward navigation history. Tracks the
//! user's path across sidebar-tab switches and detail open/close, with a
//! cursor that walks the history when Mouse-4 / Mouse-5 (or any future
//! back/forward trigger) fires.
//!
//! Two-tier model:
//!
//! * [`NavEntry`] is a snapshot of the *visible state*: a sidebar
//!   section, the tab within it for a section built out of tabs, and the
//!   optional detail id open there.
//! * [`NavHistory`] is a linear stack with a cursor; user-initiated
//!   navigation truncates the forward portion (standard HTML5 History
//!   API semantics).
//!
//! Recording happens at each user-action call site (sidebar click,
//! grid-card click, cross-tab nav, back-button click). The replay path
//! ([`replay`]) drives the same callbacks, so a single `suppress` flag
//! gates the synchronous portion to keep replay-triggered callbacks
//! from re-pushing the entry we just walked to. The async `open_*`
//! futures are *not* recording sites in this design, so the suppress
//! lifetime stays bounded to one synchronous replay step.
//!
//! Not persisted: each launch starts with an empty history. The boot
//! state is seeded synchronously at the end of [`crate::boot::ui_setup`]
//! `::install_views` (one entry for the section the user landed on),
//! and any persisted detail's async `seed_detail_from_settings` open
//! records itself on the way in.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, Weak};

use crate::state::AppState;
use crate::ui::my_library::{MyLibraryTab, NAV_MY_LIBRARY, tab_from_index};
use crate::{
    AlbumDetail, AppWindow, ArtistDetail, Dialog, GenreDetail, MyLibrary, Nav, NavEnterFrom,
    PlaylistDetail, Queue,
};

const HISTORY_CAP: usize = 24;

/// [`NavEntry::tab`] for a section that has no tabs. Only My Library does, and only
/// its four grid tabs carry a detail view.
const NO_TAB: i32 = -1;

/// One snapshot of the visible state. `detail_id == None` means the
/// section's grid/list is showing; `Some(id)` means the section's detail
/// view is open on that id.
///
/// **A section alone stopped identifying a view when the five library pages became
/// tabs of one.** `tab` is the second half: `MyLibrary.tab-idx` inside section 3,
/// [`NO_TAB`] everywhere else. Without it a Songs → Albums move is a duplicate of the
/// entry it followed, which `record`'s dedup drops on the floor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NavEntry {
    pub section: i32,
    pub tab: i32,
    pub detail_id: Option<i64>,
}

/// Linear back/forward stack with a cursor. See module docs.
pub struct NavHistory {
    entries: VecDeque<NavEntry>,
    cursor: usize,
    suppress: bool,
}

impl NavHistory {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(HISTORY_CAP),
            cursor: 0,
            suppress: false,
        }
    }

    /// Push a new entry at `cursor + 1`, truncating any forward history.
    /// No-op while a replay is in flight. Consecutive duplicates collapse.
    pub fn record(&mut self, entry: NavEntry) {
        if self.suppress {
            return;
        }
        if self.entries.is_empty() {
            self.entries.push_back(entry);
            self.cursor = 0;
            return;
        }
        if self.entries.get(self.cursor) == Some(&entry) {
            return;
        }
        // User-initiated navigation invalidates the forward stack.
        self.entries.truncate(self.cursor + 1);
        self.entries.push_back(entry);
        if self.entries.len() > HISTORY_CAP {
            self.entries.pop_front();
        }
        self.cursor = self.entries.len() - 1;
    }

    /// Move cursor one step back; return the entry now under the cursor,
    /// or `None` if already at the start.
    pub fn back(&mut self) -> Option<NavEntry> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.entries.get(self.cursor).copied()
    }

    /// Move cursor one step forward; return the entry now under the
    /// cursor, or `None` if already at the end.
    pub fn forward(&mut self) -> Option<NavEntry> {
        if self.cursor + 1 >= self.entries.len() {
            return None;
        }
        self.cursor += 1;
        self.entries.get(self.cursor).copied()
    }

    pub fn set_suppress(&mut self, on: bool) {
        self.suppress = on;
    }

    /// The entry the cursor is on, or `None` if the history is empty.
    /// Read by [`spawn_open_detail`]'s spawned future to bail when a newer
    /// replay has already moved the cursor past the target this future
    /// was scheduled to fulfil.
    pub fn current(&self) -> Option<NavEntry> {
        self.entries.get(self.cursor).copied()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }
}

impl Default for NavHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// The tab `section` currently has mounted, or [`NO_TAB`] for a section without tabs.
fn current_tab(ui: &AppWindow, section: i32) -> i32 {
    if section == NAV_MY_LIBRARY {
        ui.global::<MyLibrary>().get_tab_idx()
    } else {
        NO_TAB
    }
}

/// Read the current visible detail id for `(section, tab)` from the per-view
/// Slint globals, or `None` if that view has no detail concept or its detail
/// is closed.
///
/// **The tab is what discriminates, not the id.** `seed_detail_from_settings` runs
/// for all four detail views at boot whichever section is restored, so more than one
/// `*Detail.*-id` can be `>= 0` at a time.
fn current_detail_id_for(ui: &AppWindow, section: i32, tab: i32) -> Option<i64> {
    if section != NAV_MY_LIBRARY {
        return None;
    }
    match tab_from_index(&ui.global::<MyLibrary>(), tab) {
        MyLibraryTab::Songs => None,
        MyLibraryTab::Albums => {
            let id = i64::from(ui.global::<AlbumDetail>().get_album_id());
            (id >= 0).then_some(id)
        }
        MyLibraryTab::Artists => {
            let id = i64::from(ui.global::<ArtistDetail>().get_artist_id());
            (id >= 0).then_some(id)
        }
        MyLibraryTab::Genres => {
            let id = i64::from(ui.global::<GenreDetail>().get_genre_id());
            (id >= 0).then_some(id)
        }
        MyLibraryTab::Playlists => {
            let id = i64::from(ui.global::<PlaylistDetail>().get_playlist_id());
            (id >= 0).then_some(id)
        }
    }
}

/// Record an arbitrary `(section, detail_id)` snapshot. Used by detail
/// open/close hooks where the new state is known at the call site.
pub fn record(state: &AppState, entry: NavEntry) {
    state.nav_history.lock().record(entry);
}

/// Record the current visible state by reading globals. Cheap. Used by
/// the sidebar persist hook where the *new* state is already in the
/// globals by the time the callback fires.
pub fn record_current(state: &AppState, ui: &AppWindow) {
    let section = ui.global::<Nav>().get_selected_index();
    let tab = current_tab(ui, section);
    let detail_id = current_detail_id_for(ui, section, tab);
    state.nav_history.lock().record(NavEntry { section, tab, detail_id });
}

/// Walk one step back or forward and drive the UI into the new state.
/// All replay-side writes are wrapped in `set_suppress(true)` so the
/// transition hooks they trigger don't re-record. Suppress is cleared
/// after the synchronous portion of the work; the async `open_*` future
/// scheduled below isn't itself a recording site, so no re-suppress is
/// required.
pub fn replay(state: &AppState, ui: &AppWindow, going_back: bool) {
    let target = {
        let mut hist = state.nav_history.lock();
        if going_back { hist.back() } else { hist.forward() }
    };
    let Some(target) = target else {
        return;
    };

    let current_section = ui.global::<Nav>().get_selected_index();
    let current_tab = current_tab(ui, current_section);
    let current_detail = current_detail_id_for(ui, current_section, current_tab);
    let direction = if going_back { NavEnterFrom::Left } else { NavEnterFrom::Right };

    state.nav_history.lock().set_suppress(true);

    if target.section == current_section && target.tab == current_tab {
        apply_same_view(
            state,
            ui,
            target.section,
            target.tab,
            current_detail,
            target.detail_id,
            direction,
        );
    } else if target.section == current_section {
        // Same section, different tab — a My Library move. Neither arm below covers
        // it: the same-view one only opens or closes a detail, and the cross-section
        // one flips `Nav.selected-index`, which here would be a no-op. Tear down what
        // the leaving tab has open, move the tab, then open the *target tab's* detail.
        if current_detail.is_some() {
            invoke_close_detail(ui, current_section, current_tab);
        }
        crate::ui::nav_transition::mark(ui, direction);
        apply_tab(ui, target.tab);
        if let Some(id) = target.detail_id {
            spawn_open_detail(state, ui, target.section, target.tab, id, direction);
        }
    } else {
        // Cross-section: tear down the current detail (if any) first so
        // its release/persist side-effects run, then flip section, then
        // re-open the target detail (if any).
        if current_detail.is_some() {
            invoke_close_detail(ui, current_section, current_tab);
        }
        crate::ui::nav_transition::mark(ui, direction);
        // Before the section flip, so the page mounts on the tab it is meant to be on
        // rather than on whichever one it was left at.
        apply_tab(ui, target.tab);
        let nav = ui.global::<Nav>();
        nav.set_selected_index(target.section);
        nav.invoke_persist_selected_index(target.section);
        if let Some(id) = target.detail_id {
            spawn_open_detail(state, ui, target.section, target.tab, id, direction);
        }
    }

    state.nav_history.lock().set_suppress(false);
}

fn apply_same_view(
    state: &AppState,
    ui: &AppWindow,
    section: i32,
    tab: i32,
    current: Option<i64>,
    target: Option<i64>,
    direction: NavEnterFrom,
) {
    match (current, target) {
        (Some(_), None) => invoke_close_detail(ui, section, tab),
        (_, Some(id)) => spawn_open_detail(state, ui, section, tab, id, direction),
        (None, None) => crate::ui::nav_transition::mark(ui, direction),
    }
}

/// Move the My Library tab and remember it — the `Nav.selected-index` /
/// `persist-selected-index` pair one level down, and deliberately not `tab-changed`,
/// which is a *pick* and clears the filter. A no-op for [`NO_TAB`], i.e. for every
/// section that has no tabs.
fn apply_tab(ui: &AppWindow, tab: i32) {
    if tab < 0 {
        return;
    }
    let g = ui.global::<MyLibrary>();
    g.set_tab_idx(tab);
    g.invoke_persist_tab_idx(tab);
}

fn invoke_close_detail(ui: &AppWindow, section: i32, tab: i32) {
    if section != NAV_MY_LIBRARY {
        return;
    }
    let tab = tab_from_index(&ui.global::<MyLibrary>(), tab);
    crate::ui::my_library::close_open_detail(ui, tab);
}

/// Schedule the tab's `open_*` future and bail at the start if a
/// newer replay has already advanced the cursor past `expected`.
///
/// The bail closes the spam-click window where Mouse-4/Mouse-5 pressed
/// faster than the DB fetch returns would otherwise leave a stale
/// `*-id` set on the destination section's global — invisible while a
/// different section is selected, but ready to pop a phantom detail
/// the next time the user navigates back to that tab. A residual race
/// remains for a press that lands *after* the future starts awaiting
/// (the DB fetch can't be cancelled mid-flight); closing that one
/// would need a real cancellation token plumbed through `open_*`.
fn spawn_open_detail(
    state: &AppState,
    ui: &AppWindow,
    section: i32,
    tab: i32,
    id: i64,
    direction: NavEnterFrom,
) {
    if section != NAV_MY_LIBRARY {
        return;
    }
    let weak: Weak<AppWindow> = ui.as_weak();
    let s = state.clone();
    let expected = NavEntry { section, tab, detail_id: Some(id) };
    match tab_from_index(&ui.global::<MyLibrary>(), tab) {
        MyLibraryTab::Songs => {}
        MyLibraryTab::Albums => {
            let Some(au) = state.ui_handles.albums.lock().clone() else { return };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                if let Err(e) =
                    crate::ui::albums::open_album(&s, &au, weak, id, direction).await
                {
                    log::warn!("nav_history::replay open_album({id}): {e}");
                }
            });
        }
        MyLibraryTab::Artists => {
            let Some(au) = state.ui_handles.artists.lock().clone() else { return };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                if let Err(e) =
                    crate::ui::artists::open_artist(&s, &au, weak, id, direction).await
                {
                    log::warn!("nav_history::replay open_artist({id}): {e}");
                }
            });
        }
        MyLibraryTab::Genres => {
            let Some(gu) = state.ui_handles.genres.lock().clone() else { return };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                if let Err(e) =
                    crate::ui::genres::open_genre(&s, &gu, weak, id, direction).await
                {
                    log::warn!("nav_history::replay open_genre({id}): {e}");
                }
            });
        }
        MyLibraryTab::Playlists => {
            let Some(pu) = state.ui_handles.playlists.lock().clone() else { return };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                if let Err(e) =
                    crate::ui::playlists::open_playlist(&s, &pu, weak, id, direction).await
                {
                    log::warn!("nav_history::replay open_playlist({id}): {e}");
                }
            });
        }
    }
}

/// True when an overlay (Now Playing fullscreen, Queue sheet, or any
/// modal Dialog) is open. Mouse-4/5 are gated on this — overlays have
/// their own input contexts (Esc/F close them) and absorbing browser-
/// style nav while one is up would surprise the user.
pub fn overlay_open(ui: &AppWindow) -> bool {
    ui.global::<Nav>().get_now_playing_open()
        || ui.global::<Queue>().get_open()
        || ui.global::<Dialog>().get_open()
}

/// Per-section UI-handle registry, populated at startup by each per-view
/// `wire_*` once its `Arc<*Ui>` exists. Replay reads from this when
/// dispatching `open_*`. `Mutex<Option<...>>` so the registry can be
/// populated lazily during boot from the UI thread and read from the UI
/// thread later without threading the handles through every callback.
#[derive(Default)]
pub struct UiHandles {
    pub albums: Mutex<Option<Arc<crate::ui::albums::AlbumsUi>>>,
    pub artists: Mutex<Option<Arc<crate::ui::artists::ArtistsUi>>>,
    pub genres: Mutex<Option<Arc<crate::ui::genres::GenresUi>>>,
    pub playlists: Mutex<Option<Arc<crate::ui::playlists::PlaylistsUi>>>,
}

#[cfg(test)]
#[path = "tests/nav_history_tests.rs"]
mod tests;
