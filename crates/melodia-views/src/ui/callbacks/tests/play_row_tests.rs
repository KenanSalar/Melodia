//! Tests for the shared callback helpers in `super` — [`super::play_row_start`],
//! the `play-row` start-slot resolver, and [`super::next_sort`], the sort-pill
//! toggle every sortable view routes through.

use super::{next_sort, next_sort_with_natural, play_row_start};
use melodia_app::services::settings::SortDir;

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

/// The two moves a sort row can make, in one place now that twelve sort rows
/// share it.
#[test]
fn clicking_the_active_field_flips_the_direction() {
    let (field, dir) = next_sort("name", "asc", "name");
    assert_eq!((field.as_str(), dir.as_str()), ("name", "desc"));

    let (field, dir) = next_sort("name", "desc", "name");
    assert_eq!((field.as_str(), dir.as_str()), ("name", "asc"));
}

#[test]
fn clicking_a_new_field_starts_it_ascending() {
    // Regardless of the direction the *previous* field was carrying — the arrow
    // belongs to the field, not to the row.
    for dir in ["asc", "desc"] {
        let (field, new_dir) = next_sort("name", dir, "track_count");
        assert_eq!((field.as_str(), new_dir.as_str()), ("track_count", "asc"));
    }
}

/// `SortDir::from_token` treats anything that isn't `"desc"` as ascending, and
/// the flip agrees — so an unrecognised token flips to descending like the
/// ascending state it parses as. Testing for `"asc"` instead reads identically
/// and leaves that pill unable to reach descending at all.
#[test]
fn an_unrecognised_direction_flips_like_the_ascending_it_parses_as() {
    let parsed = SortDir::from_token("sideways");
    assert_eq!(parsed.as_str(), "asc", "from_token's rule is the one being matched");

    let (_, dir) = next_sort("name", "sideways", "name");
    assert_eq!(dir.as_str(), "desc");
}

/// Playlist Detail's full cycle. The third step is the one that matters: its
/// `"position"` order is what drag-to-reorder is gated on and no header cell
/// asks for it, so without a way back one click retired reordering for good.
#[test]
fn a_natural_order_takes_the_third_click() {
    let natural = Some("position");

    let (field, dir) = next_sort_with_natural("position", "asc", "title", natural);
    assert_eq!((field.as_str(), dir.as_str()), ("title", "asc"));

    let (field, dir) = next_sort_with_natural("title", "asc", "title", natural);
    assert_eq!((field.as_str(), dir.as_str()), ("title", "desc"));

    let (field, dir) = next_sort_with_natural("title", "desc", "title", natural);
    assert_eq!((field.as_str(), dir.as_str()), ("position", "asc"));
}

/// The eleven views that pass `None` keep the two-state flip — a sort pill has
/// no natural order to name and no way to paint one.
#[test]
fn without_a_natural_order_the_third_click_flips_back_to_ascending() {
    let (field, dir) = next_sort_with_natural("name", "desc", "name", None);
    assert_eq!((field.as_str(), dir.as_str()), ("name", "asc"));
}
