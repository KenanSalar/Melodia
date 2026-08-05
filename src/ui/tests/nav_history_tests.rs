use crate::ui::my_library::NAV_MY_LIBRARY;
use crate::ui::nav_history::{NavEntry, NavHistory};

// `MyLibrary`'s `tab-*` values, restated here because a unit test has no `AppWindow` to
// read them off. `ui::my_library::tests` pins the Slint side against the tab count.
const SONGS: i32 = 0;
const ALBUMS: i32 = 1;
const ARTISTS: i32 = 2;
const PLAYLISTS: i32 = 4;

// Sections that carry no tabs, so no detail either.
const FAVORITES: i32 = 2;
const BROWSE: i32 = 1;
const NO_TAB: i32 = -1;

fn tab(tab: i32, detail: Option<i64>) -> NavEntry {
    NavEntry { section: NAV_MY_LIBRARY, tab, detail_id: detail }
}

fn section(section: i32) -> NavEntry {
    NavEntry { section, tab: NO_TAB, detail_id: None }
}

#[test]
fn first_record_seeds_history_and_cursor() {
    let mut h = NavHistory::new();
    assert_eq!(h.len(), 0);
    h.record(tab(ALBUMS, None));
    assert_eq!(h.len(), 1);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.current(), Some(tab(ALBUMS, None)));
}

#[test]
fn consecutive_duplicate_record_dedups() {
    let mut h = NavHistory::new();
    h.record(tab(ALBUMS, None));
    h.record(tab(ALBUMS, None));
    h.record(tab(ALBUMS, None));
    assert_eq!(h.len(), 1);
    assert_eq!(h.cursor(), 0);
}

/// The whole reason [`NavEntry`] carries a tab. Every My Library tab shares nav index
/// 3, so without it these two snapshots are equal and `record`'s dedup drops the second
/// — leaving Mouse-4 unable to walk back across a tab switch.
#[test]
fn a_tab_switch_is_not_a_duplicate_of_the_tab_it_left() {
    let mut h = NavHistory::new();
    h.record(tab(SONGS, None));
    h.record(tab(ALBUMS, None));
    assert_eq!(h.len(), 2);
    assert_eq!(h.back(), Some(tab(SONGS, None)));
}

#[test]
fn distinct_record_advances_cursor() {
    let mut h = NavHistory::new();
    h.record(tab(ALBUMS, None));
    h.record(tab(ALBUMS, Some(7)));
    h.record(tab(SONGS, None));
    assert_eq!(h.len(), 3);
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.current(), Some(tab(SONGS, None)));
}

#[test]
fn back_and_forward_walk_cursor() {
    let mut h = NavHistory::new();
    h.record(tab(ALBUMS, None));
    h.record(tab(ALBUMS, Some(7)));
    h.record(tab(SONGS, None));

    assert_eq!(h.back(), Some(tab(ALBUMS, Some(7))));
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.back(), Some(tab(ALBUMS, None)));
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.back(), None, "cannot back past the start");
    assert_eq!(h.cursor(), 0, "cursor stays at 0 on overshoot");

    assert_eq!(h.forward(), Some(tab(ALBUMS, Some(7))));
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.forward(), Some(tab(SONGS, None)));
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.forward(), None, "cannot forward past the end");
    assert_eq!(h.cursor(), 2);
}

#[test]
fn user_navigation_truncates_forward_stack() {
    let mut h = NavHistory::new();
    h.record(tab(ALBUMS, None));
    h.record(tab(ALBUMS, Some(7)));
    h.record(tab(SONGS, None));
    // Walk back twice, leaving cursor=0 with 2 entries forward of it.
    h.back();
    h.back();
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.len(), 3);

    // A new user navigation while cursor is not at the tail truncates
    // the forward stack and appends after the cursor.
    h.record(tab(ARTISTS, None));
    assert_eq!(h.len(), 2, "forward entries dropped");
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.current(), Some(tab(ARTISTS, None)));
    assert_eq!(h.forward(), None, "no forward after truncation");
}

#[test]
fn suppress_blocks_record() {
    let mut h = NavHistory::new();
    h.record(tab(ALBUMS, None));
    h.set_suppress(true);
    h.record(tab(SONGS, None));
    h.record(tab(PLAYLISTS, None));
    assert_eq!(h.len(), 1, "no records while suppressed");
    assert_eq!(h.current(), Some(tab(ALBUMS, None)));
    h.set_suppress(false);
    h.record(tab(SONGS, None));
    assert_eq!(h.len(), 2);
    assert_eq!(h.current(), Some(tab(SONGS, None)));
}

#[test]
fn cap_evicts_from_front_and_pins_cursor_to_tail() {
    let mut h = NavHistory::new();
    // Push 30 distinct entries; cap is 24.
    for i in 0..30 {
        h.record(tab(ALBUMS, Some(i)));
    }
    assert_eq!(h.len(), 24, "len clamped to HISTORY_CAP");
    assert_eq!(h.cursor(), 23, "cursor sits at the tail");
    assert_eq!(h.current(), Some(tab(ALBUMS, Some(29))));
    // Oldest 6 entries (ids 0..=5) were dropped; first surviving is id 6.
    for _ in 0..23 {
        h.back();
    }
    assert_eq!(h.current(), Some(tab(ALBUMS, Some(6))));
    assert_eq!(h.back(), None);
}

#[test]
fn detail_close_after_open_records_then_back_reopens() {
    let mut h = NavHistory::new();
    h.record(tab(ALBUMS, None));
    h.record(tab(ALBUMS, Some(42)));
    h.record(tab(ALBUMS, None)); // close-detail records grid state
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.back(), Some(tab(ALBUMS, Some(42))));
    assert_eq!(h.forward(), Some(tab(ALBUMS, None)));
}

#[test]
fn cross_section_walk() {
    let mut h = NavHistory::new();
    h.record(tab(PLAYLISTS, Some(5))); // seeded boot state
    h.record(section(BROWSE));
    h.record(section(FAVORITES));
    h.record(tab(ALBUMS, Some(11)));

    // Walk all the way back to the seed.
    assert_eq!(h.back(), Some(section(FAVORITES)));
    assert_eq!(h.back(), Some(section(BROWSE)));
    assert_eq!(h.back(), Some(tab(PLAYLISTS, Some(5))));
    assert_eq!(h.back(), None);

    // Forward all the way to the latest.
    assert_eq!(h.forward(), Some(section(BROWSE)));
    assert_eq!(h.forward(), Some(section(FAVORITES)));
    assert_eq!(h.forward(), Some(tab(ALBUMS, Some(11))));
    assert_eq!(h.forward(), None);
}
