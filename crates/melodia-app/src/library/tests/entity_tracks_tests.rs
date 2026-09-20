//! The order and dedupe guarantees a card's right-click action queues under.
//!
//! The four functions here are the pure half; the `ORDER BY` each query hands them is
//! `queries/tests/entity_ids_tests.rs`'s, and how that order is *chosen* is
//! `crates/melodia/tests/cross_tier.rs`'s. Asking any of those three questions twice is how the
//! three would come to disagree.

use std::collections::HashMap;

use super::{EntityKind, dedupe, flatten_in_order, group_by_entity};

#[test]
fn every_menu_token_resolves_to_one_kind() {
    let cases = [
        ("album", Some(EntityKind::Album)),
        ("artist", Some(EntityKind::Artist)),
        ("genre", Some(EntityKind::Genre)),
        ("playlist", Some(EntityKind::Playlist)),
        ("track", Some(EntityKind::Track)),
        // A real `card-kind`, and the one invalid token that has to stay invalid: Browse routes a
        // folder through its own global, a folder being a path rather than an id.
        ("folder", None),
        ("", None),
        ("Album", None),
        ("albums", None),
    ];
    for (token, expected) in cases {
        assert_eq!(EntityKind::from_token(token), expected, "token {token:?}");
    }
}

#[test]
fn the_entities_flatten_in_the_order_they_were_asked_for() {
    // Ids out of numeric order, and a map whose own iteration order is not the answer.
    let grouped = HashMap::from([(7, vec![70, 71]), (3, vec![30]), (5, vec![50, 51])]);
    assert_eq!(flatten_in_order(grouped, &[5, 3, 7]), [50, 51, 30, 70, 71]);
}

#[test]
fn each_entity_keeps_its_own_track_order() {
    let grouped = HashMap::from([(1, vec![30, 10, 20])]);
    assert_eq!(flatten_in_order(grouped, &[1]), [30, 10, 20]);
}

#[test]
fn a_track_behind_two_selected_entities_is_queued_once() {
    // At the position of the first entity that named it, which is what a second selected artist
    // crediting the same track must not move.
    let grouped = HashMap::from([(1, vec![10, 20]), (2, vec![20, 30])]);
    assert_eq!(flatten_in_order(grouped, &[1, 2]), [10, 20, 30]);
}

#[test]
fn an_entity_with_no_tracks_contributes_nothing() {
    let grouped = HashMap::from([(1, vec![10]), (2, Vec::new()), (3, vec![30])]);
    assert_eq!(flatten_in_order(grouped, &[1, 2, 3]), [10, 30]);
}

#[test]
fn an_entity_missing_from_the_map_is_skipped() {
    // A card deleted between the grid painting and the click: the rest of the selection still acts.
    let grouped = HashMap::from([(1, vec![10])]);
    assert_eq!(flatten_in_order(grouped, &[1, 99]), [10]);
}

#[test]
fn a_repeated_entity_id_contributes_its_tracks_once() {
    let grouped = HashMap::from([(1, vec![10, 11])]);
    assert_eq!(flatten_in_order(grouped, &[1, 1]), [10, 11]);
}

#[test]
fn group_by_entity_keeps_the_row_order_the_query_returned() {
    let grouped = group_by_entity(vec![(1, 30), (2, 40), (1, 10), (1, 20)]);
    assert_eq!(grouped.get(&1).map(Vec::as_slice), Some([30, 10, 20].as_slice()));
    assert_eq!(grouped.get(&2).map(Vec::as_slice), Some([40].as_slice()));
}

#[test]
fn a_track_selection_keeps_each_id_once_at_its_first_position() {
    assert_eq!(dedupe([30, 10, 30, 20, 10].into_iter()), [30, 10, 20]);
}

#[test]
fn an_empty_selection_flattens_to_nothing() {
    assert_eq!(flatten_in_order(HashMap::from([(1, vec![10])]), &[]), Vec::<i64>::new());
    assert_eq!(dedupe(std::iter::empty()), Vec::<i64>::new());
}
