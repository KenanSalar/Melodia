use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint::{Model, SharedString, VecModel};

use super::*;
use crate::NotificationRow;

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
