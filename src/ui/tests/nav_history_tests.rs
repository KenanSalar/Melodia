use crate::ui::nav_history::{NavEntry, NavHistory};

const TRACKS: i32 = 3;
const ALBUMS: i32 = 4;
const ARTISTS: i32 = 5;
const PLAYLISTS: i32 = 7;

fn entry(section: i32, detail: Option<i64>) -> NavEntry {
    NavEntry { section, detail_id: detail }
}

#[test]
fn first_record_seeds_history_and_cursor() {
    let mut h = NavHistory::new();
    assert_eq!(h.len(), 0);
    h.record(entry(ALBUMS, None));
    assert_eq!(h.len(), 1);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.current(), Some(entry(ALBUMS, None)));
}

#[test]
fn consecutive_duplicate_record_dedups() {
    let mut h = NavHistory::new();
    h.record(entry(ALBUMS, None));
    h.record(entry(ALBUMS, None));
    h.record(entry(ALBUMS, None));
    assert_eq!(h.len(), 1);
    assert_eq!(h.cursor(), 0);
}

#[test]
fn distinct_record_advances_cursor() {
    let mut h = NavHistory::new();
    h.record(entry(ALBUMS, None));
    h.record(entry(ALBUMS, Some(7)));
    h.record(entry(TRACKS, None));
    assert_eq!(h.len(), 3);
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.current(), Some(entry(TRACKS, None)));
}

#[test]
fn back_and_forward_walk_cursor() {
    let mut h = NavHistory::new();
    h.record(entry(ALBUMS, None));
    h.record(entry(ALBUMS, Some(7)));
    h.record(entry(TRACKS, None));

    assert_eq!(h.back(), Some(entry(ALBUMS, Some(7))));
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.back(), Some(entry(ALBUMS, None)));
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.back(), None, "cannot back past the start");
    assert_eq!(h.cursor(), 0, "cursor stays at 0 on overshoot");

    assert_eq!(h.forward(), Some(entry(ALBUMS, Some(7))));
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.forward(), Some(entry(TRACKS, None)));
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.forward(), None, "cannot forward past the end");
    assert_eq!(h.cursor(), 2);
}

#[test]
fn user_navigation_truncates_forward_stack() {
    let mut h = NavHistory::new();
    h.record(entry(ALBUMS, None));
    h.record(entry(ALBUMS, Some(7)));
    h.record(entry(TRACKS, None));
    // Walk back twice, leaving cursor=0 with 2 entries forward of it.
    h.back();
    h.back();
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.len(), 3);

    // A new user navigation while cursor is not at the tail truncates
    // the forward stack and appends after the cursor.
    h.record(entry(ARTISTS, None));
    assert_eq!(h.len(), 2, "forward entries dropped");
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.current(), Some(entry(ARTISTS, None)));
    assert_eq!(h.forward(), None, "no forward after truncation");
}

#[test]
fn suppress_blocks_record() {
    let mut h = NavHistory::new();
    h.record(entry(ALBUMS, None));
    h.set_suppress(true);
    h.record(entry(TRACKS, None));
    h.record(entry(PLAYLISTS, None));
    assert_eq!(h.len(), 1, "no records while suppressed");
    assert_eq!(h.current(), Some(entry(ALBUMS, None)));
    h.set_suppress(false);
    h.record(entry(TRACKS, None));
    assert_eq!(h.len(), 2);
    assert_eq!(h.current(), Some(entry(TRACKS, None)));
}

#[test]
fn cap_evicts_from_front_and_pins_cursor_to_tail() {
    let mut h = NavHistory::new();
    // Push 30 distinct entries; cap is 24.
    for i in 0..30 {
        h.record(entry(ALBUMS, Some(i)));
    }
    assert_eq!(h.len(), 24, "len clamped to HISTORY_CAP");
    assert_eq!(h.cursor(), 23, "cursor sits at the tail");
    assert_eq!(h.current(), Some(entry(ALBUMS, Some(29))));
    // Oldest 6 entries (ids 0..=5) were dropped; first surviving is id 6.
    for _ in 0..23 {
        h.back();
    }
    assert_eq!(h.current(), Some(entry(ALBUMS, Some(6))));
    assert_eq!(h.back(), None);
}

#[test]
fn detail_close_after_open_records_then_back_reopens() {
    let mut h = NavHistory::new();
    h.record(entry(ALBUMS, None));
    h.record(entry(ALBUMS, Some(42)));
    h.record(entry(ALBUMS, None)); // close-detail records grid state
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.back(), Some(entry(ALBUMS, Some(42))));
    assert_eq!(h.forward(), Some(entry(ALBUMS, None)));
}

#[test]
fn cross_section_walk() {
    let mut h = NavHistory::new();
    h.record(entry(PLAYLISTS, Some(5))); // seeded boot state
    h.record(entry(TRACKS, None));
    h.record(entry(ALBUMS, Some(11)));

    // Walk all the way back to the seed.
    assert_eq!(h.back(), Some(entry(TRACKS, None)));
    assert_eq!(h.back(), Some(entry(PLAYLISTS, Some(5))));
    assert_eq!(h.back(), None);

    // Forward all the way to the latest.
    assert_eq!(h.forward(), Some(entry(TRACKS, None)));
    assert_eq!(h.forward(), Some(entry(ALBUMS, Some(11))));
    assert_eq!(h.forward(), None);
}
