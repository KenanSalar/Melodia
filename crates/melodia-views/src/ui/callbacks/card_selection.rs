//! Multi-selection for the entity card grids, shared by all seven of them.
//!
//! One `(scope, set)` pair rather than a copy of the state per view. Every callback carries the
//! asking grid's scope, so a grid that is not the one holding the set reads back an empty
//! selection and replaces it on its first pick, which is what stops a stale set bleeding from one
//! grid onto another whatever any mount forgets. The section leaves still hand it back, for the
//! different question `card-selection.slint` argues: a set no grid is showing.
//!
//! Not [`crate::ui::list_selection`]: that layer anchors a shift-range on a **row index**, which a
//! grid cannot offer. Its cards live in chunked rows and a re-sort moves every position, so the
//! anchor here is an **id** and the range resolves both endpoints against the model at the moment
//! of the click.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use melodia_ui::{
    Albums, AppWindow, Artists, CardSelection, Favorites, Genres, Playlists, RecentlyPlayed,
};

/// The scope token each grid passes. The `.slint` mounts spell the same literals; a mount that
/// spells something else selects nothing rather than selecting the wrong grid's cards.
///
/// **A typo degrades to a single card; the empty string used to select normally.** Only the
/// second needs stating. `EntityCardGrid.selection-scope` has no default, so a forgotten binding
/// arrives as `""`, which is also what an untouched [`Selection`] holds, and comparing equal is
/// exactly what a scope guard must not do: a grid that never named itself got a working-looking
/// set, and two of them would have shared one. [`unnamed_scope`] is the guard, asked by the two
/// callbacks that can seat a scope.
pub mod scope {
    pub const ALBUMS: &str = "albums";
    pub const ARTISTS: &str = "artists";
    pub const GENRES: &str = "genres";
    pub const PLAYLISTS: &str = "playlists";
    pub const FAVORITE_MOST_PLAYED: &str = "fav-most-played";
    pub const FAVORITE_ARTISTS: &str = "fav-artists";
    pub const RECENT_MOST_PLAYED: &str = "rp-most-played";
}

/// Whether the asking grid named no scope at all, which no selection may be seated under.
fn unnamed_scope(asking_scope: &str) -> bool {
    asking_scope.is_empty()
}

/// Which grid owns the live selection, and what is in it.
///
/// `RefCell` rather than a mutex: every path here is the event loop, as with the section gate's
/// latch, and a re-entrant borrow should fail loudly rather than park the UI thread.
#[derive(Default)]
struct Selection {
    scope: String,
    /// **Pick order, and never sorted — it is what the batch actions queue in.**
    /// `library::entity_tracks` flattens the ids in the order it is handed them, so a set sorted
    /// anywhere on the way out plays albums by database id under a grid the user sorted by year. A
    /// shift range lands in displayed order and a ctrl-click appends, which is the same `Vec` and
    /// the same reason [`crate::ui::list_selection`] has one for the track lists.
    ids: Vec<i32>,
    /// `ids` as a set, because the membership question is asked far more often than it is
    /// answered: every mounted card re-runs `selected` on each generation bump, so a scan of
    /// `ids` makes one publish cost mounted cards times selected ids. Rebuilt in [`publish`],
    /// which already walks the whole set.
    members: HashSet<i32>,
    /// The id a shift-range measures from, `0` for none. No card carries id 0.
    anchor: i32,
}

impl Selection {
    /// The only way to build one, so `members` cannot drift from `ids`.
    fn new(scope: String, ids: Vec<i32>, anchor: i32) -> Self {
        let members = ids.iter().copied().collect();
        Self { scope, ids, members, anchor }
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
            // Resolved before the write, so the borrow is closed by the time `publish` takes one.
            let next = pick(&ui, &selection.borrow(), asking_scope.as_str(), id, shift, ctrl);
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
            // The anchor is left where the last pick put it, so a shift-pick after Select All
            // still ranges from what the user chose; `list_selection::select_all_curated` argues
            // it for the track lists. **Only within the scope that set it**: `pick` drops the
            // anchor on a scope change and this is the one writer that could carry one across,
            // where entity ids all start at 1 and the next shift-range would measure from
            // whichever card happens to share the number.
            let anchor = {
                let current = selection.borrow();
                if current.scope == scope { current.anchor } else { 0 }
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

/// The new selection for a click, or `None` where there is nothing to change.
///
/// Plain click selects the one card and moves the anchor, ctrl toggles it, shift takes the range
/// from the anchor to it in displayed order. A click in a scope that isn't the live one starts
/// that scope's selection fresh, whatever the modifiers say.
fn pick(
    ui: &AppWindow,
    current: &Selection,
    asking_scope: &str,
    id: i32,
    shift: bool,
    ctrl: bool,
) -> Option<Selection> {
    if id == 0 || unnamed_scope(asking_scope) {
        return None;
    }
    let same_scope = current.scope == asking_scope;
    // Owned per branch rather than once above them: the plain-click path takes `single`'s own
    // copy, so a binding up here is an allocation it never reads.
    let single = |anchor| Selection::new(asking_scope.to_owned(), vec![id], anchor);

    if shift && same_scope && current.anchor != 0 {
        let visible = visible_ids(ui, asking_scope);
        let from = visible.iter().position(|&card| card == current.anchor);
        let to = visible.iter().position(|&card| card == id);
        // A missing endpoint means the grid re-filtered under the anchor. Fall back to the one
        // card rather than taking a range measured against a list nobody is looking at.
        let (Some(from), Some(to)) = (from, to) else {
            return Some(single(id));
        };
        let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
        return Some(Selection::new(
            asking_scope.to_owned(),
            visible[lo..=hi].to_vec(),
            current.anchor,
        ));
    }

    if ctrl && same_scope {
        let mut ids = current.ids.clone();
        match ids.iter().position(|&card| card == id) {
            Some(at) => {
                ids.remove(at);
            }
            None => ids.push(id),
        }
        return Some(Selection::new(asking_scope.to_owned(), ids, id));
    }

    Some(single(id))
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

/// The ids a scope's grid is currently drawing, in card order.
///
/// Read off the Slint model rather than any slice's own cache. The model *is* what is on screen,
/// so a range can't disagree with it mid-debounce, and this names no view's internals.
fn visible_ids(ui: &AppWindow, asking_scope: &str) -> Vec<i32> {
    match asking_scope {
        scope::ALBUMS => flatten(&ui.global::<Albums>().get_grid_rows(), |r| r.albums, |c| c.id),
        scope::ARTISTS => flatten(&ui.global::<Artists>().get_grid_rows(), |r| r.artists, |c| c.id),
        scope::GENRES => flatten(&ui.global::<Genres>().get_grid_rows(), |r| r.genres, |c| c.id),
        scope::PLAYLISTS => {
            flatten(&ui.global::<Playlists>().get_grid_rows(), |r| r.playlists, |c| c.id)
        }
        scope::FAVORITE_MOST_PLAYED => {
            flatten(&ui.global::<Favorites>().get_most_played_rows(), |r| r.entities, |c| c.id)
        }
        scope::FAVORITE_ARTISTS => {
            flatten(&ui.global::<Favorites>().get_artist_rows(), |r| r.entities, |c| c.id)
        }
        scope::RECENT_MOST_PLAYED => {
            flatten(&ui.global::<RecentlyPlayed>().get_most_played_rows(), |r| r.entities, |c| c.id)
        }
        _ => Vec::new(),
    }
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
