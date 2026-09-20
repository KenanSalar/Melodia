//! The card-grid pick matrix: scope guard, anchor, range and toggle.
//!
//! Everything a card's click decides is here; what a *published* selection then does to the Slint
//! globals needs a window and is left to the wiring. The anchor sentinel is `0`, against
//! `list_selection`'s `-1`, because this layer ranges over ids where that one ranges over row
//! indices.

use std::collections::HashSet;

use super::scope;
use super::{Selection, pick};

/// Card order as the grids draw it, for every range case below.
const VISIBLE: [i32; 5] = [10, 20, 30, 40, 50];

fn visible() -> Vec<i32> {
    VISIBLE.to_vec()
}

/// A live selection in `scope`, anchored on `anchor`.
fn live(scope: &str, ids: &[i32], anchor: i32) -> Selection {
    Selection::new(scope.to_owned(), ids.to_vec(), anchor)
}

/// Unwrap a pick that was expected to change something. `None` becomes a selection no branch can
/// produce, so the caller's own assertion is what reports the miss.
fn picked(next: Option<Selection>) -> Selection {
    next.unwrap_or_else(|| Selection::new("no pick was made".to_owned(), Vec::new(), -1))
}

/// `EntityCardGrid.selection-scope` has no Slint default, so a mount that forgets it arrives as
/// `""`, which is also an untouched [`Selection`]'s scope. Comparing equal is exactly what a
/// scope guard must not do: two unnamed grids would have shared one set.
#[test]
fn a_grid_that_named_no_scope_selects_nothing() {
    let next = pick(&Selection::default(), "", 10, false, false, visible);
    assert!(next.is_none(), "an unnamed scope must seat no selection at all");
}

#[test]
fn card_id_zero_picks_nothing() {
    let next = pick(&Selection::default(), scope::ALBUMS, 0, false, false, visible);
    assert!(next.is_none(), "no card carries id 0");
}

#[test]
fn a_plain_click_replaces_the_set_and_moves_the_anchor() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[10, 20], 10), scope::ALBUMS, 40, false, false, visible));
    assert_eq!(next.ids, [40]);
    assert_eq!(next.anchor, 40);
    assert_eq!(next.scope, scope::ALBUMS);
}

/// The scope guard's whole job, and it has to hold whatever the modifiers say: a set the user can
/// no longer see must not answer for the grid they are clicking in.
#[test]
fn a_click_in_another_scope_starts_that_scope_fresh() {
    let held = live(scope::ALBUMS, &[10, 20, 30], 10);

    let shifted = picked(pick(&held, scope::ARTISTS, 40, true, false, visible));
    assert_eq!(shifted.ids, [40]);
    assert_eq!(shifted.scope, scope::ARTISTS);

    let ctrled = picked(pick(&held, scope::ARTISTS, 40, false, true, visible));
    assert_eq!(ctrled.ids, [40]);
    assert_eq!(ctrled.scope, scope::ARTISTS);
}

#[test]
fn ctrl_adds_an_unpicked_card_at_the_end() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[30, 10], 10), scope::ALBUMS, 50, false, true, visible));
    // Pick order, never sorted: `entity_tracks::flatten_in_order` queues in exactly this order.
    assert_eq!(next.ids, [30, 10, 50]);
}

#[test]
fn ctrl_removes_a_picked_card() {
    let next = picked(pick(
        &live(scope::ALBUMS, &[30, 10, 50], 10),
        scope::ALBUMS,
        10,
        false,
        true,
        visible,
    ));
    assert_eq!(next.ids, [30, 50]);
}

#[test]
fn ctrl_moves_the_anchor_to_the_card_it_toggled() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[10], 10), scope::ALBUMS, 40, false, true, visible));
    assert_eq!(next.anchor, 40);
}

#[test]
fn shift_takes_the_range_in_displayed_order() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[20], 20), scope::ALBUMS, 40, true, false, visible));
    assert_eq!(next.ids, [20, 30, 40]);
}

#[test]
fn shift_takes_the_range_upward_in_displayed_order_too() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[40], 40), scope::ALBUMS, 20, true, false, visible));
    assert_eq!(next.ids, [20, 30, 40], "the range is the model's order, not the click's");
}

#[test]
fn shift_keeps_the_anchor_where_the_last_pick_left_it() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[20], 20), scope::ALBUMS, 40, true, false, visible));
    assert_eq!(next.anchor, 20);
}

#[test]
fn shift_without_an_anchor_falls_back_to_the_one_card() {
    let next = picked(pick(&live(scope::ALBUMS, &[], 0), scope::ALBUMS, 30, true, false, visible));
    assert_eq!(next.ids, [30]);
    assert_eq!(next.anchor, 30);
}

/// A grid re-filtered under the anchor. Ranging anyway would measure against a list nobody is
/// looking at, so the pick degrades to the card that was actually clicked.
#[test]
fn a_range_whose_anchor_has_been_filtered_out_falls_back_to_the_one_card() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[99], 99), scope::ALBUMS, 30, true, false, visible));
    assert_eq!(next.ids, [30]);
    assert_eq!(next.anchor, 30);
}

#[test]
fn a_range_onto_a_card_the_grid_no_longer_draws_falls_back_too() {
    let next =
        picked(pick(&live(scope::ALBUMS, &[20], 20), scope::ALBUMS, 99, true, false, visible));
    assert_eq!(next.ids, [99]);
}

/// Flattening the chunked grid models is the only cost a pick can carry, and only a range owes it.
#[test]
fn the_visible_walk_is_only_paid_for_by_a_range() {
    let held = live(scope::ALBUMS, &[10], 10);

    let plain = pick(&held, scope::ALBUMS, 40, false, false, || {
        unreachable!("a plain click must not walk the grid models")
    });
    assert_eq!(picked(plain).ids, [40]);

    let toggled = pick(&held, scope::ALBUMS, 40, false, true, || {
        unreachable!("a ctrl-click must not walk the grid models")
    });
    assert_eq!(picked(toggled).ids, [10, 40]);
}

/// `members` answers every mounted card's `selected` binding on every generation bump, so a drift
/// from `ids` is a grid painting one set and queueing another. `Selection::new` is the single
/// assembly point that makes it impossible; this is what holds every branch to going through it.
#[test]
fn the_members_set_never_drifts_from_the_picked_ids() {
    let held = live(scope::ALBUMS, &[20, 40], 20);
    let cases = [
        ("plain", picked(pick(&held, scope::ALBUMS, 30, false, false, visible)), vec![30]),
        (
            "ctrl add",
            picked(pick(&held, scope::ALBUMS, 30, false, true, visible)),
            vec![20, 40, 30],
        ),
        ("ctrl remove", picked(pick(&held, scope::ALBUMS, 20, false, true, visible)), vec![40]),
        (
            "shift range",
            picked(pick(&held, scope::ALBUMS, 40, true, false, visible)),
            vec![20, 30, 40],
        ),
        ("cross scope", picked(pick(&held, scope::GENRES, 30, true, false, visible)), vec![30]),
        ("default", Selection::default(), Vec::new()),
    ];
    for (branch, next, expected) in cases {
        assert_eq!(next.ids, expected, "{branch}");
        assert_eq!(next.members, expected.iter().copied().collect::<HashSet<i32>>(), "{branch}");
    }
}
