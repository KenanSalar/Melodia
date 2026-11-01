use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{Model, SharedString};

use super::*;

/// A retired row's removal fires only when a test calls [`run_exits`], so until then it stays,
/// `leaving`: read [`live`].
fn make_ui() -> NotificationsUi {
    NotificationsUi::new(Duration::ZERO)
}

/// With no platform installed, Slint fires only zero-duration timers, which is every exit
/// [`make_ui`]'s rows wait on.
fn run_exits() {
    slint::platform::update_timers_and_animations();
}

/// The rows whose cards are still up, read as the card reads them.
fn live(ui: &NotificationsUi) -> Vec<NotificationRow> {
    ui.rows.iter().filter(|row| !row.leaving).collect()
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
    let id0 = ui.show(make_params(NotificationKind::None), STICKY);
    let id1 = ui.show(make_params(NotificationKind::None), STICKY);
    let id2 = ui.show(make_params(NotificationKind::None), STICKY);
    assert_eq!(id0, 0);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(ui.rows.row_count(), 3);
}

#[test]
fn dismiss_retires_only_the_matching_row() {
    let ui = make_ui();
    let id0 = ui.show(make_params(NotificationKind::None), STICKY);
    let id1 = ui.show(make_params(NotificationKind::None), STICKY);
    let id2 = ui.show(make_params(NotificationKind::None), STICKY);

    ui.dismiss(id1);

    let ids: Vec<i32> = live(&ui).iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![id0, id2]);
}

/// The card fades and closes over its row, so the row has to outlive the retire.
#[test]
fn a_retired_row_stays_in_the_model_while_its_card_closes() {
    let ui = make_ui();
    let kept = ui.show(make_params(NotificationKind::None), STICKY);
    let retired = ui.show(make_params(NotificationKind::None), STICKY);

    ui.dismiss(retired);

    let rows: Vec<(i32, bool)> = ui.rows.iter().map(|r| (r.id, r.leaving)).collect();
    assert_eq!(rows, [(kept, false), (retired, true)]);
}

/// A row nothing drops is a closed card the stack holds for the rest of the session.
#[test]
fn a_retired_row_is_dropped_once_its_exit_has_run() {
    let ui = make_ui();
    let kept = ui.show(make_params(NotificationKind::None), STICKY);
    let retired = ui.show(make_params(NotificationKind::None), STICKY);
    ui.dismiss(retired);

    run_exits();

    let ids: Vec<i32> = ui.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, [kept]);
}

/// An exit lands after the rows ahead of it may have gone, so one taken by the position it was
/// retired at drops a live card instead. Retired out of order, so neither order the exits fire in
/// hides that.
#[test]
fn each_retired_row_is_dropped_by_its_own_id() {
    let ui = make_ui();
    let first = ui.show(make_params(NotificationKind::None), STICKY);
    let second = ui.show(make_params(NotificationKind::None), STICKY);
    let third = ui.show(make_params(NotificationKind::None), STICKY);
    let kept = ui.show(make_params(NotificationKind::None), STICKY);
    let last_kept = ui.show(make_params(NotificationKind::None), STICKY);
    ui.dismiss(second);
    ui.dismiss(first);
    ui.dismiss(third);

    run_exits();

    let ids: Vec<i32> = ui.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, [kept, last_kept]);
}

#[test]
fn dismiss_unknown_id_is_noop() {
    let ui = make_ui();
    ui.show(make_params(NotificationKind::None), STICKY);
    ui.dismiss(999);
    assert_eq!(ui.rows.row_count(), 1);
}

#[test]
fn dismiss_by_kind_retires_every_matching_row() {
    let ui = make_ui();
    ui.show(make_params(NotificationKind::WatcherDisabled), STICKY);
    ui.show(make_params(NotificationKind::InstallUpdate), STICKY);
    ui.show(make_params(NotificationKind::WatcherDisabled), STICKY);
    ui.show(make_params(NotificationKind::UpdateFailed), STICKY);

    ui.dismiss_by_kind(NotificationKind::WatcherDisabled);

    let kinds: Vec<NotificationKind> = live(&ui).iter().map(|r| r.kind).collect();
    assert_eq!(kinds, [NotificationKind::InstallUpdate, NotificationKind::UpdateFailed]);
}

#[test]
fn dismiss_by_kind_no_match_is_noop() {
    let ui = make_ui();
    ui.show(make_params(NotificationKind::InstallUpdate), STICKY);
    ui.show(make_params(NotificationKind::UpdateFailed), STICKY);
    ui.dismiss_by_kind(NotificationKind::WatcherDisabled);
    assert_eq!(ui.rows.row_count(), 2);
}

#[test]
fn max_visible_evicts_oldest_on_overflow() {
    let ui = make_ui();
    let ids: Vec<i32> = (0..=super::MAX_VISIBLE)
        .map(|_| ui.show(make_params(NotificationKind::None), STICKY))
        .collect();

    let live_ids: Vec<i32> = live(&ui).iter().map(|r| r.id).collect();
    assert_eq!(live_ids, &ids[1..]);
}

/// A leaving card is already on its way off the stack, so counting it evicts a live one early.
#[test]
fn a_leaving_row_leaves_room_under_the_cap() {
    let ui = make_ui();
    let ids: Vec<i32> = (0..super::MAX_VISIBLE)
        .map(|_| ui.show(make_params(NotificationKind::None), STICKY))
        .collect();
    ui.dismiss(ids[0]);

    let newest = ui.show(make_params(NotificationKind::None), STICKY);

    let live_ids: Vec<i32> = live(&ui).iter().map(|r| r.id).collect();
    assert_eq!(live_ids, [&ids[1..], &[newest]].concat());
}

/// A stub recipe. Never run — these pins are about the map's *lifetime*, and rendering
/// needs an `AppWindow` that a unit test has no way to build.
fn stub_recipe() -> Relabel {
    Box::new(|_ui: &AppWindow| RowText::plain(SharedString::default(), SharedString::default()))
}

/// Push a row and register a recipe against it, the two halves `show_localized` pairs.
fn show_with_recipe(ui: &NotificationsUi, kind: NotificationKind) -> i32 {
    let id = ui.show(make_params(kind), STICKY);
    ui.recipes.borrow_mut().insert(id, stub_recipe());
    id
}

/// A recipe is a closure kept for the session, and ids are monotonic and never reused, so a
/// recipe outliving its row is a leak nothing else can collect. Each way out calls `retire_at` on
/// its own, so each gets a pin, the eviction inside `show` most of all: no caller sees it.
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
    ui.show(make_params(NotificationKind::None), STICKY);

    assert!(ui.recipes.borrow().is_empty());
}

/// Takes the row it is handed off the stack, one way or another.
type Retirement = fn(&NotificationsUi, i32);

/// Every id the row-retired listener hears, in order.
fn listening(ui: &NotificationsUi) -> Rc<RefCell<Vec<i32>>> {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let sink = heard.clone();
    ui.on_row_retired(move |id| sink.borrow_mut().push(id));
    heard
}

/// A path that misses the listener leaves Ctrl+Z undoing a removal whose notice is gone.
#[test]
fn every_way_a_row_goes_tells_the_listener_once() {
    let rows: [(&str, Retirement); 3] = [
        ("a dismiss", |ui, id| ui.dismiss(id)),
        ("a dismiss by kind", |ui, _| ui.dismiss_by_kind(NotificationKind::Undo)),
        ("the cap evicting it", |ui, _| {
            for _ in 0..super::MAX_VISIBLE {
                ui.show(make_params(NotificationKind::None), STICKY);
            }
        }),
    ];
    for (what, retire) in rows {
        let ui = make_ui();
        let heard = listening(&ui);
        let id = ui.show(make_params(NotificationKind::Undo), STICKY);

        retire(&ui, id);

        assert_eq!(*heard.borrow(), [id], "{what}");
    }
}

/// The listener hears a row once, and a leaving row still sits in the model for every way out to
/// find, so each has to read it as gone.
#[test]
fn a_leaving_row_is_retired_only_once() {
    let rows: [(&str, Retirement); 2] = [
        ("a second dismiss", |ui, id| ui.dismiss(id)),
        ("a dismiss by kind", |ui, _| ui.dismiss_by_kind(NotificationKind::Undo)),
    ];
    for (what, retire_again) in rows {
        let ui = make_ui();
        let heard = listening(&ui);
        let id = ui.show(make_params(NotificationKind::Undo), STICKY);
        ui.dismiss(id);

        retire_again(&ui, id);

        assert_eq!(*heard.borrow(), [id], "{what} retired a row that was already leaving");
    }
}

/// The card starts its clock over when `restarts` moves, which is how a merged removal earns the
/// full wait again. The id stays, the undo slot holding its row by it.
#[test]
fn a_restart_moves_the_clock_and_keeps_the_row() {
    let ui = make_ui();
    let id = ui.show(make_params(NotificationKind::Undo), 6_000);

    ui.restart(id);

    let row = ui.rows.row_data(0).map(|row| (row.id, row.restarts, row.timeout_ms));
    assert_eq!(row, Some((id, 1, 6_000)));
}
