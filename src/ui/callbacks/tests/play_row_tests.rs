//! Tests for [`super::play_row_start`] — the `play-row` start-slot resolver.

use super::play_row_start;

#[test]
fn aligned_index_is_taken_as_is() {
    let ids = [10, 20, 30, 40];
    assert_eq!(play_row_start(&ids, 30, 2), Some(2));
}

#[test]
fn misaligned_index_falls_back_to_lookup_by_id() {
    // Browse shape: the displayed list interleaves disk-only rows, so the
    // clicked index sits past the track's slot in the in-library id list.
    let ids = [10, 20, 30];
    assert_eq!(play_row_start(&ids, 20, 3), Some(1));
}

#[test]
fn negative_index_falls_back_to_lookup_by_id() {
    let ids = [10, 20, 30];
    assert_eq!(play_row_start(&ids, 30, -1), Some(2));
}

#[test]
fn unknown_track_starts_at_the_head() {
    let ids = [10, 20, 30];
    assert_eq!(play_row_start(&ids, 99, 1), None);
}

#[test]
fn empty_list_starts_at_the_head() {
    assert_eq!(play_row_start(&[], 10, 0), None);
}
