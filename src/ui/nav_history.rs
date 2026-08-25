//! Browser-style in-memory back/forward navigation history, tracking the user's path
//! across sidebar-tab switches and detail open/close, walked by Mouse-4 / Mouse-5.
//!
//! [`NavEntry`] is a snapshot of the *visible state* — a section, the tab within it for
//! a section built out of tabs, and the optional detail id open there. [`NavHistory`] is
//! a linear stack with a cursor, where user-initiated navigation truncates the forward
//! portion, as the HTML5 History API does.
//!
//! Recording happens at each user-action call site. The replay path drives the same
//! callbacks, so a single `suppress` flag keeps a replay-triggered callback from
//! re-pushing the entry it just walked to. It is a plain bool rather than a counter, so
//! **the two regions it guards must not nest**: `replay`'s own synchronous step, and the
//! [`PendingNav`] a cross-view walk defers into the destination `open_*`'s hook, which
//! lands a section flip — and so a `record_current` — long after that step returned.
//! [`PendingNav::apply`] and [`PendingNav::apply_deferred`] are that split.
//!
//! Not persisted: each launch starts empty. `boot::ui_setup::install_views` seeds one
//! entry for the section landed on, and any persisted detail's async open records itself
//! on the way in.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, Weak};

use crate::state::AppState;
use crate::ui::my_library::{
    self, MyLibraryTab, NAV_MY_LIBRARY, persist_tab, tab_from_index, tab_of_section,
};
use crate::ui::radio::NAV_RADIO;
use crate::ui::view_tag;
use crate::{AppWindow, Dialog, MyLibrary, Nav, NavEnterFrom, Queue, Radio};

const HISTORY_CAP: usize = 24;

/// One snapshot of the visible state. `detail_id == None` means the section's grid or
/// list is showing, `Some(id)` its detail view.
///
/// **A section alone stopped identifying a view when the five library pages became tabs
/// of one.** `tab` is the second half — `MyLibrary.tab-idx` inside section 3,
/// [`crate::ui::my_library::NO_TAB`] everywhere else. Without it a Songs → Albums move
/// duplicates the entry it followed and `record`'s dedup drops it.
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

    /// Push a new entry at `cursor + 1`, truncating any forward history. A no-op while a
    /// replay is in flight; consecutive duplicates collapse.
    ///
    /// Returns whether the entry was taken, which is what [`record_current`] logs on:
    /// the eleven hooks fire two or three times per move, so an unconditional line reads
    /// as a stutter rather than as a navigation.
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
        // User-initiated navigation invalidates the forward stack.
        self.entries.truncate(self.cursor + 1);
        self.entries.push_back(entry);
        if self.entries.len() > HISTORY_CAP {
            self.entries.pop_front();
        }
        self.cursor = self.entries.len() - 1;
        true
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

    /// Drop every entry for a section the user can no longer reach, pulling the cursor
    /// back past the ones that fell before it.
    ///
    /// Switching Radio off is the only caller, and it is what actually moves the user off
    /// nav 10: the row and the router branch both go, but the walk would still land there
    /// on a panel with nothing mounted in it. Cheaper than teaching every replay to
    /// re-check whether its destination still exists.
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

    /// The entry the cursor is on, or `None` if the history is empty. Read by
    /// [`spawn_open_detail`]'s future to bail when a newer replay has already moved the
    /// cursor past the target it was scheduled to fulfil.
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

/// The visible detail id for `(section, tab)`, or `None` if that view has no detail
/// concept or its detail is closed. Past the My Library check everything is
/// [`my_library::detail_id_for`] — which takes the tab rather than reading the mounted one
/// precisely so this caller can ask about a *recorded* entry's tab.
///
/// **Radio's detail is walkable only where it has a row.** A browsed station is a directory
/// answer with a shelf life and `id == 0`, so there is nothing for a later replay to reopen
/// it by; the walk steps over it to the tab it was opened from, which is where the station
/// still is.
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

/// Record an arbitrary snapshot, for the detail open/close hooks where the new state is
/// known at the call site.
pub fn record(state: &AppState, entry: NavEntry) {
    state.nav_history.lock().record(entry);
}

/// Record the current visible state by reading globals, for the hooks that fire once the
/// *new* state is already there.
pub fn record_current(state: &AppState, ui: &AppWindow) {
    let section = ui.global::<Nav>().get_selected_index();
    let tab = tab_of_section(ui, section);
    let detail_id = current_detail_id_for(ui, section, tab);
    // Only what the history took: the eleven hooks fire two or three deep for one click,
    // and logging each reads as a stutter rather than a navigation.
    if state.nav_history.lock().record(NavEntry {
        section,
        tab,
        detail_id,
    }) {
        view_tag::log_current(ui);
    }
}

/// Walk one step back or forward and drive the UI into the new state, under
/// `set_suppress(true)` so the transition hooks it triggers don't re-record. Suppress is
/// cleared after the synchronous portion — but a cross-view walk into a detail defers
/// its navigation into the destination `open_*`'s hook, which lands a section flip long
/// after that, so [`PendingNav::apply_deferred`] re-arms the flag around itself. The two
/// regions must not nest; see the module docs.
pub fn replay(state: &AppState, ui: &AppWindow, going_back: bool) {
    let target = {
        let mut hist = state.nav_history.lock();
        if going_back {
            hist.back()
        } else {
            hist.forward()
        }
    };
    let Some(target) = target else {
        return;
    };

    let current_section = ui.global::<Nav>().get_selected_index();
    let current_tab = tab_of_section(ui, current_section);
    let current_detail = current_detail_id_for(ui, current_section, current_tab);
    let direction = if going_back {
        NavEnterFrom::Left
    } else {
        NavEnterFrom::Right
    };

    // Its own line because a replay suppresses recording, so this is otherwise
    // the one navigation leaving no trace.
    log::debug!(
        "nav: history {} → section {} tab {} detail {:?}",
        if going_back { "back" } else { "forward" },
        target.section,
        target.tab,
        target.detail_id
    );

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
    } else {
        // A different *view* — a My Library tab move, a section switch, or both. Four
        // things are owed either way: tear down whatever the leaving view has open so
        // its release and persist side-effects run, mark the direction, land on the
        // target tab, and put the section where the target names.
        //
        // **Whether those run now or later turns on whether a detail is coming.**
        // `my-library-view.slint`'s body router is a pure function of `(tab-idx, the
        // four ids)` with no third state, and the id lands a DB fetch plus an artwork
        // decode after the tab would — so a tab written here mounts the destination's
        // *grid* for the length of that fetch. Handed to `open_*_with`'s hook instead,
        // both land in one UI-thread tick and the router only ever sees the detail.
        let pending = PendingNav {
            from: current_detail.is_some().then_some((current_section, current_tab)),
            section: target.section,
            tab: target.tab,
            direction,
        };
        if let Some(id) = target.detail_id {
            spawn_open_detail(state, ui, target.section, target.tab, id, direction, Some(pending));
        } else {
            // Nothing to wait for — the destination view *is* the grid.
            pending.apply(ui);
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
        // No navigation to defer — the view the detail belongs to is already the one
        // on screen, so the hook has nothing to land and `open_*` marks for itself.
        (_, Some(id)) => spawn_open_detail(state, ui, section, tab, id, direction, None),
        (None, None) => crate::ui::nav_transition::mark(ui, direction),
    }
}

/// The navigation a cross-view replay owes, deferred so it lands in the same UI-thread
/// tick as the destination detail's id rather than a fetch ahead of it. The call site in
/// [`replay`] says why the two must share a tick.
#[derive(Clone, Copy)]
struct PendingNav {
    /// The view being left, when it has a detail to close.
    from: Option<(i32, i32)>,
    section: i32,
    tab: i32,
    direction: NavEnterFrom,
}

impl PendingNav {
    /// Land it. For [`replay`]'s own synchronous arm, which is already inside
    /// the suppression — the deferred callers take [`Self::apply_deferred`].
    fn apply(self, ui: &AppWindow) {
        if let Some((section, tab)) = self.from {
            invoke_close_detail(ui, section, tab);
        }
        // **After the close**, which runs `mark_drill_back` and would otherwise clobber
        // the direction `open_*` marked a few statements above this hook.
        crate::ui::nav_transition::mark(ui, self.direction);
        // Before the section flip, so the page mounts on the tab it is meant to be on
        // rather than on whichever one it was left at.
        persist_tab(ui, self.tab);
        // **Read the index live, after the close.** "Did this walk cross a section" is a
        // different question from "is the section where the target names", and only the
        // second is the flip's: a detail opened by a cross-section drill carries an
        // `origin-nav-index` and closing it *restores that section*, so a same-section
        // walk out of one has already left the page by now. The live read still skips
        // the redundant persist on an ordinary same-section move.
        let nav = ui.global::<Nav>();
        if nav.get_selected_index() != self.section {
            nav.set_selected_index(self.section);
            nav.invoke_persist_selected_index(self.section);
        }
    }

    /// The deferred form, for the two callers that run after [`replay`] has returned:
    /// the `open_*` hook and [`Self::land`]. Suppression is dropped by then and the
    /// section flip inside reaches the sidebar hook's `record_current`, which would push
    /// the entry the walk just walked *to*. `record`'s dedup swallows it today;
    /// re-arming is two lines and stops the fix resting on that.
    fn apply_deferred(self, state: &AppState, ui: &AppWindow) {
        state.nav_history.lock().set_suppress(true);
        self.apply(ui);
        state.nav_history.lock().set_suppress(false);
    }

    /// Land it on an event-loop hop of its own, for the paths where the open never ran
    /// or failed. Without it a press against a deleted playlist — or one arriving before
    /// the view handles are wired — does nothing at all, the whole navigation having
    /// been handed to a hook that never fires.
    fn land(self, state: &AppState, weak: &Weak<AppWindow>) {
        let state = state.clone();
        let _ = weak.upgrade_in_event_loop(move |ui| self.apply_deferred(&state, &ui));
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

/// Schedule the tab's `open_*` future, bailing at the start if a newer replay has
/// already advanced the cursor past `expected`.
///
/// That bail closes the spam-click window: pressed faster than the DB fetch returns,
/// Mouse-4/5 would otherwise leave a stale `*-id` on the destination global — invisible
/// while another section is selected, ready to pop a phantom detail on the next visit. A
/// residual race remains for a press landing *after* the future starts awaiting, which
/// would need a real cancellation token plumbed through `open_*`.
///
/// `pending` is the cross-view navigation, handed to the `open_*_with` hook so it shares
/// a tick with the id write; `None` when the target is the view already on screen.
/// **Every path that can still open something and doesn't reach the hook has to land it
/// instead**, or the press is silently swallowed — that is the missing-handle bail and
/// each open's `Err` arm. Three paths deliberately land nothing: the newer-replay bail,
/// where a later replay already owns the screen, and the two guards below, a
/// non-My-Library section and the Songs tab owning no detail at all.
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
    // Two handles: `weak` is consumed by whichever `open_*_with` runs, `fallback` is what
    // is left to land the navigation on if it never gets there.
    let weak: Weak<AppWindow> = ui.as_weak();
    let fallback: Weak<AppWindow> = ui.as_weak();
    let s = state.clone();
    let expected = NavEntry {
        section,
        tab,
        detail_id: Some(id),
    };
    if section == NAV_RADIO {
        let Some(ru) = state.ui_handles.radio.lock().clone() else {
            land_pending(pending, state, &fallback);
            return;
        };
        // Only a kept station is ever recorded, so the uuid is the row's to look up and this
        // one is left empty — `StationRef::is_kept` splits on the id.
        let station = crate::ui::radio::StationRef {
            id,
            uuid: String::new(),
        };
        state.runtime.clone().spawn(async move {
            if s.nav_history.lock().current() != Some(expected) {
                return;
            }
            let hook = pending_hook(pending, &s);
            if let Err(e) =
                crate::ui::radio::open_station_with(&s, &ru, weak, station, direction, hook).await
            {
                log::warn!("nav_history::replay open_station({id}): {e}");
                land_pending(pending, &s, &fallback);
            }
        });
        return;
    }
    match tab_from_index(&ui.global::<MyLibrary>(), tab) {
        MyLibraryTab::Songs => {}
        MyLibraryTab::Albums => {
            let Some(au) = state.ui_handles.albums.lock().clone() else {
                land_pending(pending, state, &fallback);
                return;
            };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                let hook = pending_hook(pending, &s);
                if let Err(e) =
                    crate::ui::albums::open_album_with(&s, &au, weak, id, direction, hook).await
                {
                    log::warn!("nav_history::replay open_album({id}): {e}");
                    land_pending(pending, &s, &fallback);
                }
            });
        }
        MyLibraryTab::Artists => {
            let Some(au) = state.ui_handles.artists.lock().clone() else {
                land_pending(pending, state, &fallback);
                return;
            };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                let hook = pending_hook(pending, &s);
                if let Err(e) =
                    crate::ui::artists::open_artist_with(&s, &au, weak, id, direction, hook).await
                {
                    log::warn!("nav_history::replay open_artist({id}): {e}");
                    land_pending(pending, &s, &fallback);
                }
            });
        }
        MyLibraryTab::Genres => {
            let Some(gu) = state.ui_handles.genres.lock().clone() else {
                land_pending(pending, state, &fallback);
                return;
            };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                let hook = pending_hook(pending, &s);
                if let Err(e) =
                    crate::ui::genres::open_genre_with(&s, &gu, weak, id, direction, hook).await
                {
                    log::warn!("nav_history::replay open_genre({id}): {e}");
                    land_pending(pending, &s, &fallback);
                }
            });
        }
        MyLibraryTab::Playlists => {
            let Some(pu) = state.ui_handles.playlists.lock().clone() else {
                land_pending(pending, state, &fallback);
                return;
            };
            state.runtime.clone().spawn(async move {
                if s.nav_history.lock().current() != Some(expected) {
                    return;
                }
                let hook = pending_hook(pending, &s);
                if let Err(e) =
                    crate::ui::playlists::open_playlist_with(&s, &pu, weak, id, direction, hook)
                        .await
                {
                    log::warn!("nav_history::replay open_playlist({id}): {e}");
                    land_pending(pending, &s, &fallback);
                }
            });
        }
    }
}

/// The `on_applied` hook the four `open_*_with` calls share — a no-op when the
/// replay has no navigation to defer.
fn pending_hook(
    pending: Option<PendingNav>,
    state: &AppState,
) -> impl FnOnce(&AppWindow) + Send + 'static {
    let state = state.clone();
    move |ui: &AppWindow| {
        if let Some(pending) = pending {
            pending.apply_deferred(&state, ui);
        }
    }
}

/// [`PendingNav::land`] over the same `Option`, so the bails read as one line.
fn land_pending(pending: Option<PendingNav>, state: &AppState, weak: &Weak<AppWindow>) {
    if let Some(pending) = pending {
        pending.land(state, weak);
    }
}

/// Whether Now Playing, the queue sheet or a modal dialog is up. Mouse-4/5 gate on it:
/// overlays have their own input contexts, Esc and F closing them, and absorbing
/// browser-style nav while one is up would surprise the user.
pub fn overlay_open(ui: &AppWindow) -> bool {
    ui.global::<Nav>().get_now_playing_open()
        || ui.global::<Queue>().get_open()
        || ui.global::<Dialog>().get_open()
}

/// Per-section UI-handle registry, filled at startup by each `wire_*` once its
/// `Arc<*Ui>` exists and read by the replay's `open_*` dispatch.
/// `Mutex<Option<…>>` so it can be populated lazily during boot rather than threading
/// the handles through every callback.
#[derive(Default)]
pub struct UiHandles {
    pub albums: Mutex<Option<Arc<crate::ui::albums::AlbumsUi>>>,
    pub artists: Mutex<Option<Arc<crate::ui::artists::ArtistsUi>>>,
    pub genres: Mutex<Option<Arc<crate::ui::genres::GenresUi>>>,
    pub playlists: Mutex<Option<Arc<crate::ui::playlists::PlaylistsUi>>>,
    pub radio: Mutex<Option<Arc<crate::ui::radio::RadioUi>>>,
}

#[cfg(test)]
#[path = "tests/nav_history_tests.rs"]
mod tests;
