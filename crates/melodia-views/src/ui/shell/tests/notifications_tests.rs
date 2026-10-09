use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint::{Model, SharedString, VecModel};

use super::*;

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

fn make_params(kind: NotificationKind) -> NotificationParams {
    NotificationParams {
        variant: NotificationVariant::Warning,
        title: SharedString::from("Title"),
        message: SharedString::from("Message"),
        action_label: SharedString::default(),
        kind,
    }
}

#[test]
fn show_appends_a_row_and_returns_monotonic_id() {
    let ui = make_ui();
    let id0 = ui.show(make_params(NotificationKind::None));
    let id1 = ui.show(make_params(NotificationKind::None));
    let id2 = ui.show(make_params(NotificationKind::None));
    assert_eq!(id0, 0);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(ui.rows.row_count(), 3);
}

#[test]
fn dismiss_removes_only_the_matching_row() {
    let ui = make_ui();
    let id0 = ui.show(make_params(NotificationKind::None));
    let id1 = ui.show(make_params(NotificationKind::None));
    let id2 = ui.show(make_params(NotificationKind::None));

    ui.dismiss(id1);

    assert_eq!(ui.rows.row_count(), 2);
    let ids: Vec<i32> = ui.rows.iter().map(|r: NotificationRow| r.id).collect();
    assert_eq!(ids, vec![id0, id2]);
}

#[test]
fn dismiss_unknown_id_is_noop() {
    let ui = make_ui();
    ui.show(make_params(NotificationKind::None));
    ui.dismiss(999);
    assert_eq!(ui.rows.row_count(), 1);
}

#[test]
fn dismiss_by_kind_removes_every_matching_row() {
    let ui = make_ui();
    ui.show(make_params(NotificationKind::WatcherDisabled));
    ui.show(make_params(NotificationKind::InstallUpdate));
    ui.show(make_params(NotificationKind::WatcherDisabled));
    ui.show(make_params(NotificationKind::UpdateFailed));

    ui.dismiss_by_kind(NotificationKind::WatcherDisabled);

    let kinds: Vec<NotificationKind> = ui.rows.iter().map(|r: NotificationRow| r.kind).collect();
    assert_eq!(kinds, [NotificationKind::InstallUpdate, NotificationKind::UpdateFailed]);
}

#[test]
fn dismiss_by_kind_no_match_is_noop() {
    let ui = make_ui();
    ui.show(make_params(NotificationKind::InstallUpdate));
    ui.show(make_params(NotificationKind::UpdateFailed));
    ui.dismiss_by_kind(NotificationKind::WatcherDisabled);
    assert_eq!(ui.rows.row_count(), 2);
}

#[test]
fn max_visible_evicts_oldest_on_overflow() {
    // Push one more than the cap and confirm the oldest (id 0) is gone
    // while the newest lands at the back.
    let ui = make_ui();
    let total = super::MAX_VISIBLE + 1;
    let ids: Vec<i32> = (0..total).map(|_| ui.show(make_params(NotificationKind::None))).collect();

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
fn show_with_recipe(ui: &NotificationsUi, kind: NotificationKind) -> i32 {
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
    let kept = show_with_recipe(&ui, NotificationKind::None);
    let gone = show_with_recipe(&ui, NotificationKind::None);

    ui.dismiss(gone);

    assert_eq!(ui.recipes.borrow().len(), 1);
    assert!(ui.recipes.borrow().contains_key(&kept));
}

#[test]
fn dismiss_by_kind_drops_every_matching_recipe() {
    let ui = make_ui();
    show_with_recipe(&ui, NotificationKind::WatcherDisabled);
    let kept = show_with_recipe(&ui, NotificationKind::InstallUpdate);
    show_with_recipe(&ui, NotificationKind::WatcherDisabled);

    ui.dismiss_by_kind(NotificationKind::WatcherDisabled);

    assert_eq!(ui.recipes.borrow().len(), 1);
    assert!(ui.recipes.borrow().contains_key(&kept));
}

#[test]
fn the_cap_eviction_drops_the_evicted_rows_recipe() {
    let ui = make_ui();
    let ids: Vec<i32> =
        (0..=super::MAX_VISIBLE).map(|_| show_with_recipe(&ui, NotificationKind::None)).collect();

    assert_eq!(ui.recipes.borrow().len(), super::MAX_VISIBLE);
    assert!(!ui.recipes.borrow().contains_key(&ids[0]));
}

#[test]
fn an_action_that_left_something_out_is_partial() {
    assert_eq!(Completion::partial_if(true), Completion::Partial);
    assert_eq!(Completion::partial_if(false), Completion::Complete);
}

/// The colour is the only thing telling an import that dropped a file apart from one that didn't,
/// the two toasts sharing a title.
#[test]
fn a_whole_completion_toasts_success_and_a_partial_one_warns() {
    let ui = make_ui();

    ui.show_completion(Completion::Complete, "Imported".into(), "3 playlists".into());
    ui.show_completion(Completion::Partial, "Imported".into(), "2 of 3 playlists".into());

    let variants: Vec<NotificationVariant> = ui.rows.iter().map(|row| row.variant).collect();
    assert_eq!(variants, [NotificationVariant::Success, NotificationVariant::Warning]);
}

/// A row pushed without a recipe — every `show_auto_dismiss` — must stay untouched by the
/// relabel walk rather than being skipped into an empty string.
#[test]
fn a_row_with_no_recipe_registers_none() {
    let ui = make_ui();
    ui.show(make_params(NotificationKind::None));

    assert!(ui.recipes.borrow().is_empty());
}
