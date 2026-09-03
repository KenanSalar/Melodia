use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint::{Model, SharedString, VecModel};

use super::*;
use melodia_ui::NotificationRow;

/// Build a `NotificationsUi` outside of an `AppWindow`. The Slint global
/// wiring in `install()` is purely about pushing the same `VecModel` into
/// the global's `rows` property — the model operations themselves are
/// pure data and don't need a live event loop.
fn make_ui() -> NotificationsUi {
    NotificationsUi {
        rows: Rc::new(VecModel::default()),
        recipes: Rc::new(RefCell::new(HashMap::new())),
        next_id: Cell::new(0),
    }
}

fn make_params(kind: &str) -> NotificationParams {
    NotificationParams {
        variant: SharedString::from("warning"),
        title: SharedString::from("Title"),
        message: SharedString::from("Message"),
        action_label: SharedString::default(),
        action_kind: SharedString::from(kind),
    }
}

#[test]
fn show_appends_a_row_and_returns_monotonic_id() {
    let ui = make_ui();
    let id0 = ui.show(make_params("a"));
    let id1 = ui.show(make_params("b"));
    let id2 = ui.show(make_params("c"));
    assert_eq!(id0, 0);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(ui.rows.row_count(), 3);
}

#[test]
fn dismiss_removes_only_the_matching_row() {
    let ui = make_ui();
    let id0 = ui.show(make_params("a"));
    let id1 = ui.show(make_params("b"));
    let id2 = ui.show(make_params("c"));

    ui.dismiss(id1);

    assert_eq!(ui.rows.row_count(), 2);
    let ids: Vec<i32> = ui.rows.iter().map(|r: NotificationRow| r.id).collect();
    assert_eq!(ids, vec![id0, id2]);
}

#[test]
fn dismiss_unknown_id_is_noop() {
    let ui = make_ui();
    ui.show(make_params("a"));
    ui.dismiss(999);
    assert_eq!(ui.rows.row_count(), 1);
}

#[test]
fn dismiss_by_kind_removes_every_matching_row() {
    let ui = make_ui();
    ui.show(make_params("watcher-disabled"));
    ui.show(make_params("update-available"));
    ui.show(make_params("watcher-disabled"));
    ui.show(make_params("install-failed"));

    ui.dismiss_by_kind("watcher-disabled");

    assert_eq!(ui.rows.row_count(), 2);
    let kinds: Vec<String> =
        ui.rows.iter().map(|r: NotificationRow| r.action_kind.to_string()).collect();
    assert_eq!(kinds, vec!["update-available", "install-failed"]);
}

#[test]
fn dismiss_by_kind_no_match_is_noop() {
    let ui = make_ui();
    ui.show(make_params("a"));
    ui.show(make_params("b"));
    ui.dismiss_by_kind("c");
    assert_eq!(ui.rows.row_count(), 2);
}

#[test]
fn max_visible_evicts_oldest_on_overflow() {
    // Push one more than the cap and confirm the oldest (id 0) is gone
    // while the newest lands at the back.
    let ui = make_ui();
    let total = super::MAX_VISIBLE + 1;
    let ids: Vec<i32> = (0..total).map(|_| ui.show(make_params("x"))).collect();

    assert_eq!(ui.rows.row_count(), super::MAX_VISIBLE);
    let live_ids: Vec<i32> = ui.rows.iter().map(|r: NotificationRow| r.id).collect();
    // The first id we pushed should be gone; the rest in order.
    assert!(!live_ids.contains(&ids[0]));
    assert_eq!(live_ids.first().copied(), Some(ids[1]));
    assert_eq!(live_ids.last().copied(), Some(ids[total - 1]));
}

/// A stub recipe. Never run — these pins are about the map's *lifetime*, and rendering
/// needs an `AppWindow` that a unit test has no way to build.
fn stub_recipe() -> Relabel {
    Box::new(|_ui: &AppWindow| RowText::plain(SharedString::default(), SharedString::default()))
}

/// Push a row and register a recipe against it, the two halves `show_localized` pairs.
fn show_with_recipe(ui: &NotificationsUi, kind: &str) -> i32 {
    let id = ui.show(make_params(kind));
    ui.recipes.borrow_mut().insert(id, stub_recipe());
    id
}

/// A recipe is a closure kept for the session, and ids are monotonic and never reused, so a
/// recipe outliving its row is a leak nothing else can collect. Each removal path is its own
/// call to `remove_at`, so each is worth its own pin — the eviction inside `show` is the one
/// that had no caller to notice it.
#[test]
fn dismiss_drops_the_rows_recipe() {
    let ui = make_ui();
    let kept = show_with_recipe(&ui, "a");
    let gone = show_with_recipe(&ui, "b");

    ui.dismiss(gone);

    assert_eq!(ui.recipes.borrow().len(), 1);
    assert!(ui.recipes.borrow().contains_key(&kept));
}

#[test]
fn dismiss_by_kind_drops_every_matching_recipe() {
    let ui = make_ui();
    show_with_recipe(&ui, "watcher-disabled");
    let kept = show_with_recipe(&ui, "install-update");
    show_with_recipe(&ui, "watcher-disabled");

    ui.dismiss_by_kind("watcher-disabled");

    assert_eq!(ui.recipes.borrow().len(), 1);
    assert!(ui.recipes.borrow().contains_key(&kept));
}

#[test]
fn the_cap_eviction_drops_the_evicted_rows_recipe() {
    let ui = make_ui();
    let ids: Vec<i32> = (0..=super::MAX_VISIBLE).map(|_| show_with_recipe(&ui, "x")).collect();

    assert_eq!(ui.recipes.borrow().len(), super::MAX_VISIBLE);
    assert!(!ui.recipes.borrow().contains_key(&ids[0]));
}

/// A row pushed without a recipe — every `show_auto_dismiss` — must stay untouched by the
/// relabel walk rather than being skipped into an empty string.
#[test]
fn a_row_with_no_recipe_registers_none() {
    let ui = make_ui();
    ui.show(make_params("a"));

    assert!(ui.recipes.borrow().is_empty());
}
