//! Browser-style back/forward history across section, tab and detail changes, walked by
//! Mouse-4 / Mouse-5. In memory only: `boot::ui_setup::views::install_views` seeds the
//! landing entry each launch.
//!
//! Recording happens at each user-action call site, and a replay drives those same
//! callbacks, so `suppress` keeps one from re-pushing the entry it just walked to. It is a
//! bool rather than a counter, so **the two regions it guards must not nest**: `replay`'s
//! own synchronous step, and the [`PendingNav`] a cross-view walk defers into the
//! destination `open_*`'s hook ([`PendingNav::apply`] and [`PendingNav::apply_deferred`]).

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use slint::{ComponentHandle, Weak};

use crate::ui::my_library::{
    self, MyLibraryTab, NAV_MY_LIBRARY, persist_tab, tab_from_index, tab_of_section,
};
use crate::ui::radio::NAV_RADIO;
use crate::ui::view_tag;
use melodia_app::state::AppState;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, Dialog, MyLibrary, Nav, NavEnterFrom, Queue, Radio};

const HISTORY_CAP: usize = 24;

/// One snapshot of the visible state; `detail_id == None` is the section's grid or list.
/// `tab` is `MyLibrary.tab-idx` inside section 3 and [`crate::ui::my_library::NO_TAB`]
/// elsewhere, without which a Songs → Albums move would dedup against the entry before it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NavEntry {
    pub section: i32,
    pub tab: i32,
    pub detail_id: Option<i64>,
}

pub struct NavHistory {
    entries: VecDeque<NavEntry>,
    cursor: usize,
    suppress: bool,
}

impl NavHistory {
    pub fn new() -> Self {
        Self { entries: VecDeque::with_capacity(HISTORY_CAP), cursor: 0, suppress: false }
    }

    /// Pushes `entry` past the cursor, dropping any forward history, as the HTML5 History
    /// API does. Consecutive duplicates collapse, and nothing is taken mid-replay.
    ///
    /// Returns whether it was taken, which is what [`record_current`] logs on: one move
    /// fires its hooks two or three deep, and a line per hook reads as a stutter.
    pub fn record(&mut self, entry: NavEntry) -> bool {
        if self.suppress {
            return false;
        }
        if self.entries.is_empty() {
            self.entries.push_back(entry);
            self.cursor = 0;
            return true;
        }
        if self.entries.get(self.cursor) == Some(&entry) {
            return false;
        }
        self.entries.truncate(self.cursor + 1);
        self.entries.push_back(entry);
        if self.entries.len() > HISTORY_CAP {
            self.entries.pop_front();
        }
        self.cursor = self.entries.len() - 1;
        true
    }

    /// Steps the cursor back, returning the entry it lands on.
    pub fn back(&mut self) -> Option<NavEntry> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.entries.get(self.cursor).copied()
    }

    /// Steps the cursor forward, returning the entry it lands on.
    pub fn forward(&mut self) -> Option<NavEntry> {
        if self.cursor + 1 >= self.entries.len() {
            return None;
        }
        self.cursor += 1;
        self.entries.get(self.cursor).copied()
    }

    /// Drops every entry for a section the user can no longer reach. Switching Radio off is
    /// the caller: the walk would otherwise land on a panel with nothing mounted in it.
    pub fn forget_section(&mut self, section: i32) {
        let dropped_before_cursor =
            self.entries.iter().take(self.cursor + 1).filter(|e| e.section == section).count();
        self.entries.retain(|e| e.section != section);
        self.cursor = self
            .cursor
            .saturating_sub(dropped_before_cursor)
            .min(self.entries.len().saturating_sub(1));
    }

    pub fn set_suppress(&mut self, on: bool) {
        self.suppress = on;
    }

    /// The entry under the cursor. [`spawn_open_detail`]'s future reads it to bail once a
    /// newer replay has moved past its target.
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

/// The visible detail id for `(section, tab)`. [`my_library::detail_id_for`] takes the tab
/// rather than reading the mounted one so a *recorded* entry's tab can be asked about.
///
/// Radio's detail is walkable only where the station has a row: a browsed one is a
/// directory answer with `id == 0`, so the walk steps over it to the tab it was opened from.
fn current_detail_id_for(ui: &AppWindow, section: i32, tab: i32) -> Option<i64> {
    if section == NAV_RADIO {
        let g = ui.global::<Radio>();
        return g
            .get_detail_open()
            .then(|| i64::from(g.get_detail_station().id))
            .filter(|id| crate::ui::radio::station_has_row(*id));
    }
    if section != NAV_MY_LIBRARY {
        return None;
    }
    my_library::detail_id_for(ui, tab_from_index(&ui.global::<MyLibrary>(), tab))
}

/// Records `entry`, for the hooks that know the new state at the call site.
pub fn record(entry: NavEntry) {
    nav().history().record(entry);
}

/// Records the visible state off the globals, for the hooks that fire once it has landed.
pub fn record_current(ui: &AppWindow) {
    let section = ui.global::<Nav>().get_selected_index();
    let tab = tab_of_section(ui, section);
    let detail_id = current_detail_id_for(ui, section, tab);
    if nav().history().record(NavEntry { section, tab, detail_id }) {
        view_tag::log_current(ui);
    }
}

/// Walks one step and drives the UI into the entry it lands on, with recording suppressed
/// (module docs).
pub fn replay(state: &AppState, ui: &AppWindow, going_back: bool) {
    let target = {
        let mut hist = nav().history();
        if going_back { hist.back() } else { hist.forward() }
    };
    let Some(target) = target else {
        return;
    };

    let current_section = ui.global::<Nav>().get_selected_index();
    let current_tab = tab_of_section(ui, current_section);
    let current_detail = current_detail_id_for(ui, current_section, current_tab);
    let direction = if going_back { NavEnterFrom::Left } else { NavEnterFrom::Right };

    // A replay records nothing, so this is its only trace.
    log::debug!(
        "nav: history {} → section {} tab {} detail {:?}",
        if going_back { "back" } else { "forward" },
        target.section,
        target.tab,
        target.detail_id
    );

    nav().history().set_suppress(true);

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
    } else {
        // With a detail coming, the navigation waits for `open_*_with`'s hook: the body
        // router in `my-library-view.slint` would otherwise mount the destination's grid
        // for the length of the fetch.
        let pending = PendingNav {
            from: current_detail.is_some().then_some((current_section, current_tab)),
            section: target.section,
            tab: target.tab,
            direction,
        };
        if let Some(id) = target.detail_id {
            spawn_open_detail(state, ui, target.section, target.tab, id, direction, Some(pending));
        } else {
            pending.apply(ui);
        }
    }

    nav().history().set_suppress(false);
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
        // Already on the detail's view, so nothing to defer; `open_*` marks for itself.
        (_, Some(id)) => spawn_open_detail(state, ui, section, tab, id, direction, None),
        (None, None) => crate::ui::nav_transition::mark(ui, direction),
    }
}

/// The navigation a cross-view replay owes, deferred to land in the same tick as the
/// destination detail's id.
#[derive(Clone, Copy)]
struct PendingNav {
    /// The view being left, when it has a detail to close.
    from: Option<(i32, i32)>,
    section: i32,
    tab: i32,
    direction: NavEnterFrom,
}

impl PendingNav {
    /// For [`replay`]'s synchronous arm, already inside the suppression.
    fn apply(self, ui: &AppWindow) {
        if let Some((section, tab)) = self.from {
            invoke_close_detail(ui, section, tab);
        }
        // After the close, whose `mark_drill_back` would clobber the direction.
        crate::ui::nav_transition::mark(ui, self.direction);
        // Before the section flip, so the page mounts on its target tab.
        persist_tab(ui, self.tab);
        // Read live, after the close: closing a detail a cross-section drill opened restores
        // its origin section, so a same-section walk out of one has already left the page.
        let nav = ui.global::<Nav>();
        if nav.get_selected_index() != self.section {
            nav.set_selected_index(self.section);
            nav.invoke_persist_selected_index(self.section);
        }
    }

    /// For the `open_*` hook and [`Self::land`], which run after [`replay`] has returned.
    /// Re-arms suppression, or the section flip's `record_current` would push the entry
    /// just walked to and leave the walk resting on `record`'s dedup.
    fn apply_deferred(self, ui: &AppWindow) {
        nav().history().set_suppress(true);
        self.apply(ui);
        nav().history().set_suppress(false);
    }

    /// Lands it on an event-loop hop of its own, for an open that never ran or failed and
    /// would otherwise swallow the press.
    fn land(self, weak: &Weak<AppWindow>) {
        let _ = weak.upgrade_in_event_loop(move |ui| self.apply_deferred(&ui));
    }
}

fn invoke_close_detail(ui: &AppWindow, section: i32, tab: i32) {
    if section == NAV_RADIO {
        let g = ui.global::<Radio>();
        if g.get_detail_open() {
            g.invoke_close_detail();
        }
        return;
    }
    if section != NAV_MY_LIBRARY {
        return;
    }
    let tab = tab_from_index(&ui.global::<MyLibrary>(), tab);
    crate::ui::my_library::close_open_detail(ui, tab);
}

/// Schedules the tab's `open_*`, bailing at the start if a newer replay has moved the
/// cursor past the target: spam-clicked faster than the fetch, Mouse-4/5 would otherwise
/// leave a stale `*-id` to pop a phantom detail on the next visit. A press landing after
/// the open starts awaiting still races, short of a cancellation token through `open_*`.
///
/// `pending` is the cross-view navigation, handed to the `open_*_with` hook so it shares a
/// tick with the id write. **A path that can still open something and misses the hook lands
/// it instead** (a missing handle, a failed open), or the press is swallowed. The
/// newer-replay bail and the two guards, a section with no detail and the Songs tab, land
/// nothing.
fn spawn_open_detail(
    state: &AppState,
    ui: &AppWindow,
    section: i32,
    tab: i32,
    id: i64,
    direction: NavEnterFrom,
    pending: Option<PendingNav>,
) {
    if section != NAV_MY_LIBRARY && section != NAV_RADIO {
        return;
    }
    let weak: Weak<AppWindow> = ui.as_weak();
    let replay = |label| ReplayOpen {
        expected: NavEntry { section, tab, detail_id: Some(id) },
        pending,
        fallback: ui.as_weak(),
        label,
    };
    if section == NAV_RADIO {
        // Only a kept station is recorded, so the id is the whole handle; `open_station_with`
        // fills the uuid in off the row it resolves.
        let station = crate::ui::radio::StationRef { id, uuid: String::new() };
        let handle = nav().handles().radio.lock().clone();
        spawn_open(state, handle, replay("open_station"), move |s, ru| async move {
            let hook = pending_hook(pending);
            crate::ui::radio::open_station_with(&s, &ru, weak, station, direction, hook).await
        });
        return;
    }
    match tab_from_index(&ui.global::<MyLibrary>(), tab) {
        MyLibraryTab::Songs => {}
        MyLibraryTab::Albums => {
            let handle = nav().handles().albums.lock().clone();
            spawn_open(state, handle, replay("open_album"), move |s, au| async move {
                let hook = pending_hook(pending);
                crate::ui::albums::open_album_with(&s, &au, weak, id, direction, hook).await
            });
        }
        MyLibraryTab::Artists => {
            let handle = nav().handles().artists.lock().clone();
            spawn_open(state, handle, replay("open_artist"), move |s, au| async move {
                let hook = pending_hook(pending);
                crate::ui::artists::open_artist_with(&s, &au, weak, id, direction, hook).await
            });
        }
        MyLibraryTab::Genres => {
            let handle = nav().handles().genres.lock().clone();
            spawn_open(state, handle, replay("open_genre"), move |s, gu| async move {
                let hook = pending_hook(pending);
                crate::ui::genres::open_genre_with(&s, &gu, weak, id, direction, hook).await
            });
        }
        MyLibraryTab::Playlists => {
            let handle = nav().handles().playlists.lock().clone();
            spawn_open(state, handle, replay("open_playlist"), move |s, pu| async move {
                let hook = pending_hook(pending);
                crate::ui::playlists::open_playlist_with(&s, &pu, weak, id, direction, hook).await
            });
        }
    }
}

/// One replay's open: the entry it must still be current against, the navigation it owes,
/// and the handle to land that on if the open never reaches its hook.
struct ReplayOpen {
    expected: NavEntry,
    pending: Option<PendingNav>,
    fallback: Weak<AppWindow>,
    /// Names the open in the warning a failure leaves.
    label: &'static str,
}

/// Runs `open` behind the newer-replay bail, landing `pending` wherever it misses the hook.
fn spawn_open<H, Fut>(
    state: &AppState,
    handle: Option<Arc<H>>,
    replay: ReplayOpen,
    open: impl FnOnce(AppState, Arc<H>) -> Fut + Send + 'static,
) where
    H: Send + Sync + 'static,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    let ReplayOpen { expected, pending, fallback, label } = replay;
    let Some(handle) = handle else {
        land_pending(pending, &fallback);
        return;
    };
    let s = state.clone();
    state.runtime.spawn(async move {
        if nav().history().current() != Some(expected) {
            return;
        }
        if let Err(e) = open(s, handle).await {
            let id = expected.detail_id.unwrap_or_default();
            log::warn!("nav_history::replay {label}({id}): {}", describe(&e));
            land_pending(pending, &fallback);
        }
    });
}

/// The `on_applied` hook every `open_*_with` call here shares.
fn pending_hook(pending: Option<PendingNav>) -> impl FnOnce(&AppWindow) + Send + 'static {
    move |ui: &AppWindow| {
        if let Some(pending) = pending {
            pending.apply_deferred(ui);
        }
    }
}

fn land_pending(pending: Option<PendingNav>, weak: &Weak<AppWindow>) {
    if let Some(pending) = pending {
        pending.land(weak);
    }
}

/// Whether Now Playing, a modal dialog or the floating queue is up, each owning its own
/// input, so Mouse-4/5 stand down. The docked queue panel sits beside the page and doesn't
/// count.
pub fn overlay_open(ui: &AppWindow) -> bool {
    ui.global::<Nav>().get_now_playing_open()
        || ui.global::<Dialog>().get_open()
        || ui.global::<Queue>().get_floating()
}

/// The view handles the replay's `open_*` dispatch reads, filled by each `wire_*` once its
/// `Arc<*Ui>` exists rather than threaded through every callback.
#[derive(Default)]
pub struct SectionHandles {
    pub albums: Mutex<Option<Arc<crate::ui::albums::AlbumsUi>>>,
    pub artists: Mutex<Option<Arc<crate::ui::artists::ArtistsUi>>>,
    pub genres: Mutex<Option<Arc<crate::ui::genres::GenresUi>>>,
    pub playlists: Mutex<Option<Arc<crate::ui::playlists::PlaylistsUi>>>,
    pub radio: Mutex<Option<Arc<crate::ui::radio::RadioUi>>>,
}

/// Everything browser-style navigation needs, owned by the view layer so `AppState` names
/// nothing under `ui::`.
#[derive(Default)]
pub struct Navigation {
    history: Mutex<NavHistory>,
    handles: SectionHandles,
}

impl Navigation {
    pub fn history(&self) -> parking_lot::MutexGuard<'_, NavHistory> {
        self.history.lock()
    }

    pub fn handles(&self) -> &SectionHandles {
        &self.handles
    }
}

/// A static rather than a value threaded from `boot`: the `'static` callbacks that record
/// and the `open_*_with` futures that land would each need a parameter for a value there is
/// exactly one of.
static NAVIGATION: LazyLock<Navigation> = LazyLock::new(Navigation::default);

/// The process's navigation state, fresh on first use, so there is no install to order.
pub fn nav() -> &'static Navigation {
    &NAVIGATION
}

#[cfg(test)]
#[path = "tests/nav_history_tests.rs"]
mod tests;
