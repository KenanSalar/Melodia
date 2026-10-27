//! Multi-selection for the ten card grids: one `(scope, set)` pair rather than a copy per view.
//! Every callback carries the asking grid's scope, so a grid that doesn't hold the set reads back
//! an empty selection and replaces it on its first pick, which keeps a stale set from bleeding
//! onto another grid whatever a mount forgets. The section leaves still hand it back, for the
//! question `card-selection.slint` argues: a set no grid is showing.
//!
//! The click is [`compute_click_selection`]'s, anchored on an id; a grid adds the scope and a
//! card order read off its model, a card having no flat position.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::ui::list_selection::{NO_ANCHOR, compute_click_selection};
use crate::ui::paged_rows::grid::{GridRow, PagedGrid};
use melodia_ui::{
    Albums, AppWindow, Artists, CardSelection, Favorites, Genres, Playlists, Radio, RecentlyPlayed,
};

/// The scope token each grid passes, spelled the same in the `.slint` mounts; a mount spelling
/// anything else selects nothing. **Except `""`**: `EntityCardGrid.selection-scope` has no
/// default, and a forgotten binding compares equal to an untouched [`Selection`], so two such
/// grids would share one. [`unnamed_scope`] guards the two callbacks that can seat a scope.
pub mod scope {
    pub const ALBUMS: &str = "albums";
    pub const ARTISTS: &str = "artists";
    pub const GENRES: &str = "genres";
    pub const PLAYLISTS: &str = "playlists";
    pub const FAVORITE_MOST_PLAYED: &str = "fav-most-played";
    pub const FAVORITE_ARTISTS: &str = "fav-artists";
    pub const RECENT_MOST_PLAYED: &str = "rp-most-played";
    pub const RADIO_BROWSE: &str = "radio-browse";
    pub const RADIO_FAVORITES: &str = "radio-favorites";
    pub const RADIO_RECENT: &str = "radio-recent";
}

/// Whether the asking grid named no scope at all, which no selection may be seated under.
fn unnamed_scope(asking_scope: &str) -> bool {
    asking_scope.is_empty()
}

/// Which grid owns the live selection, and what is in it.
///
/// `RefCell` rather than a mutex: every path here is the event loop, as with the section gate's
/// latch, and a re-entrant borrow should fail loudly rather than park the UI thread.
struct Selection {
    scope: String,
    /// **Pick order, never sorted, being what the batch actions queue in**:
    /// `library::entity_tracks` flattens the ids in the order handed over, so a sorted set would
    /// play albums by database id under a grid sorted by year.
    ids: Vec<i32>,
    /// `ids` as a set: every mounted card asks `selected` on each generation bump, so a scan
    /// would cost mounted cards times picks.
    members: HashSet<i32>,
    /// The id a shift-range measures from, or [`NO_ANCHOR`].
    anchor: i32,
}

impl Selection {
    /// The one place the fields are assembled, [`Default`] included, so `members` cannot drift
    /// from `ids`.
    fn new(scope: String, ids: Vec<i32>, anchor: i32) -> Self {
        let members = ids.iter().copied().collect();
        Self { scope, ids, members, anchor }
    }
}

impl Default for Selection {
    /// The empty selection: no scope, so every grid's `selected` reads back `false`.
    fn default() -> Self {
        Self::new(String::new(), Vec::new(), NO_ANCHOR)
    }
}

/// Wire the four `CardSelection` callbacks. UI-thread only, like every other selection path.
pub fn wire(ui: &AppWindow) {
    let global = ui.global::<CardSelection>();
    let weak = ui.as_weak();
    let selection = Rc::new(RefCell::new(Selection::default()));

    {
        let selection = selection.clone();
        global.on_selected(move |asking_scope, id, _generation| {
            let selection = selection.borrow();
            selection.scope == asking_scope.as_str() && selection.members.contains(&id)
        });
    }

    {
        let weak = weak.clone();
        let selection = selection.clone();
        global.on_pick(move |asking_scope, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            let scope = asking_scope.as_str();
            // Resolved before the write, so the borrow is closed by the time `publish` takes one.
            let next =
                pick(&selection.borrow(), scope, id, shift, ctrl, || visible_ids(&ui, scope));
            if let Some(next) = next {
                publish(&ui, &selection, next);
            }
        });
    }

    {
        let weak = weak.clone();
        let selection = selection.clone();
        global.on_select_all(move |asking_scope| {
            let Some(ui) = weak.upgrade() else { return };
            let scope = asking_scope.to_string();
            if unnamed_scope(&scope) {
                return;
            }
            let ids = visible_ids(&ui, &scope);
            if ids.is_empty() {
                return;
            }
            // The anchor stays where the last pick put it (`list_selection::select_all_rows`),
            // **within its own scope only**: entity ids all start at 1, so one carried across
            // would range from whichever card shares the number.
            let anchor = {
                let current = selection.borrow();
                if current.scope == scope { current.anchor } else { NO_ANCHOR }
            };
            publish(&ui, &selection, Selection::new(scope, ids, anchor));
        });
    }

    {
        let weak = weak.clone();
        global.on_clear(move || {
            let Some(ui) = weak.upgrade() else { return };
            publish(&ui, &selection, Selection::default());
        });
    }
}

/// The position a card's click passes, a card having none: the grids chunk their rows, so a range
/// finds the clicked card by its id.
const NO_POSITION: i32 = -1;

/// The new selection for a click, or `None` where there is nothing to change. A click in a scope
/// that isn't the live one starts fresh whatever the modifiers say, the set and anchor held
/// naming another grid's cards.
fn pick(
    current: &Selection,
    asking_scope: &str,
    id: i32,
    shift: bool,
    ctrl: bool,
    visible: impl FnOnce() -> Vec<i32>,
) -> Option<Selection> {
    if id == 0 || unnamed_scope(asking_scope) {
        return None;
    }
    if current.scope != asking_scope {
        return Some(Selection::new(asking_scope.to_owned(), vec![id], id));
    }
    let (ids, anchor) = compute_click_selection(
        current.anchor,
        current.ids.clone(),
        visible,
        NO_POSITION,
        id,
        shift,
        ctrl,
    );
    Some(Selection::new(asking_scope.to_owned(), ids, anchor))
}

/// Store the new selection and mirror it into Slint, bumping the generation that re-runs every
/// mounted card's `selected` binding.
fn publish(ui: &AppWindow, selection: &RefCell<Selection>, next: Selection) {
    let global = ui.global::<CardSelection>();
    let ids = next.ids.clone();
    let scope = SharedString::from(next.scope.as_str());
    *selection.borrow_mut() = next;

    let model = global.get_selected_ids();
    if let Some(vec_model) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vec_model.set_vec(ids);
    } else {
        global.set_selected_ids(ModelRc::new(VecModel::from(ids)));
    }
    global.set_scope(scope);
    global.set_generation(global.get_generation().wrapping_add(1));
}

/// The ids a scope's grid is drawing, in card order, off the Slint model rather than a slice's
/// cache, so a range can't disagree with the screen mid-debounce. A library grid's model holds
/// its ids, flattening it reading every card; Radio's are flattened.
fn visible_ids(ui: &AppWindow, asking_scope: &str) -> Vec<i32> {
    match asking_scope {
        scope::ALBUMS => card_ids(&ui.global::<Albums>().get_grid_rows(), asking_scope),
        scope::ARTISTS => card_ids(&ui.global::<Artists>().get_grid_rows(), asking_scope),
        scope::GENRES => card_ids(&ui.global::<Genres>().get_grid_rows(), asking_scope),
        scope::PLAYLISTS => card_ids(&ui.global::<Playlists>().get_grid_rows(), asking_scope),
        scope::FAVORITE_MOST_PLAYED => {
            card_ids(&ui.global::<Favorites>().get_most_played_rows(), asking_scope)
        }
        scope::FAVORITE_ARTISTS => {
            card_ids(&ui.global::<Favorites>().get_artist_rows(), asking_scope)
        }
        scope::RECENT_MOST_PLAYED => {
            card_ids(&ui.global::<RecentlyPlayed>().get_most_played_rows(), asking_scope)
        }
        scope::RADIO_BROWSE => {
            flatten(&ui.global::<Radio>().get_browse_rows(), |r| r.stations, |c| c.select_key)
        }
        scope::RADIO_FAVORITES => {
            flatten(&ui.global::<Radio>().get_favorites_rows(), |r| r.stations, |c| c.select_key)
        }
        scope::RADIO_RECENT => {
            flatten(&ui.global::<Radio>().get_recent_rows(), |r| r.stations, |c| c.select_key)
        }
        _ => Vec::new(),
    }
}

/// A paged grid's ids, as the selection names them.
fn card_ids<G: GridRow>(grid_rows: &ModelRc<G>, label: &str) -> Vec<i32> {
    PagedGrid::installed(grid_rows, label).map(PagedGrid::card_ids).unwrap_or_default()
}

/// Flatten a chunked grid model back into card order. Only the shift branch pays for it.
fn flatten<GridRow, Card>(
    grid_rows: &ModelRc<GridRow>,
    cards: impl Fn(GridRow) -> ModelRc<Card>,
    id: impl Fn(&Card) -> i32,
) -> Vec<i32>
where
    GridRow: Clone + 'static,
    Card: Clone + 'static,
{
    // Extended per row rather than `flat_map`ped: the inner `ModelRc` is a temporary, so a
    // `flat_map` has to `collect` each row into its own `Vec` to outlive it.
    let mut out = Vec::new();
    for grid_row in grid_rows.iter() {
        out.extend(cards(grid_row).iter().map(|card| id(&card)));
    }
    out
}

#[cfg(test)]
#[path = "tests/card_selection_tests.rs"]
mod tests;
