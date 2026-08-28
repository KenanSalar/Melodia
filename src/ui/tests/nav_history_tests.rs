use crate::ui::my_library::{NAV_MY_LIBRARY, NO_TAB};
use crate::ui::nav_history::{NavEntry, NavHistory};
use crate::ui::radio::NAV_RADIO;

// `MyLibrary`'s `tab-*` values, restated here because a unit test has no `AppWindow` to
// read them off. `ui::my_library::tests` pins the Slint side against the tab count.
const SONGS: i32 = 0;
const ALBUMS: i32 = 1;
const ARTISTS: i32 = 2;
const PLAYLISTS: i32 = 4;

// Sections that carry no tabs, so no detail either.
const FAVORITES: i32 = 2;
const BROWSE: i32 = 1;

// `Radio`'s `tab-*` values, restated for the same reason as My Library's above.
// `ui::radio::tests` pins the Slint side against the tab count.
const RADIO_BROWSE: i32 = 0;
const RADIO_RECENT: i32 = 2;

fn tab(tab: i32, detail: Option<i64>) -> NavEntry {
    NavEntry {
        section: NAV_MY_LIBRARY,
        tab,
        detail_id: detail,
    }
}

fn section(section: i32) -> NavEntry {
    NavEntry {
        section,
        tab: NO_TAB,
        detail_id: None,
    }
}

/// Radio is the one section `forget_section` is written for, and its entries carry a tab
/// like My Library's — so two of them differ without a detail id between them.
fn radio(tab: i32) -> NavEntry {
    NavEntry {
        section: NAV_RADIO,
        tab,
        detail_id: None,
    }
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

/// `record`'s verdict is what the verbose log's `nav:` line fires on, so it has
/// to agree with what the history did — `true` exactly when the entry landed.
///
/// The three cases are the three the log gets wrong if this drifts: a repeat
/// (the eleven hooks fire two or three deep per click), a replay in flight
/// (which logs its own line), and a real move.
#[test]
fn record_reports_whether_it_took_the_entry() {
    let mut h = NavHistory::new();
    assert!(h.record(tab(ALBUMS, None)), "the first entry lands");
    assert!(!h.record(tab(ALBUMS, None)), "a repeat collapses onto the cursor");
    assert!(h.record(tab(ARTISTS, None)), "a real move lands");

    h.set_suppress(true);
    assert!(!h.record(tab(PLAYLISTS, None)), "a replay records nothing");
    h.set_suppress(false);
    assert!(h.record(tab(PLAYLISTS, None)), "and lands once the replay is done");
}

/// Switching Radio off drops its entries, and the cursor has to come back by however many
/// of them sat behind it — otherwise the walk resumes pointing at whatever slid into the
/// index it was holding, which is a different page than the one the user was on.
///
/// **The cursor is deliberately not at the tail here.** With it on the last entry the
/// trailing `min(len - 1)` clamp lands on the right answer by itself, so a `forget` that
/// dropped the subtraction entirely would still pass — which is the whole mutation this
/// test exists to catch.
#[test]
fn forgetting_a_section_pulls_the_cursor_back_past_what_fell_before_it() {
    let mut h = NavHistory::new();
    h.record(section(BROWSE));
    h.record(radio(RADIO_BROWSE));
    h.record(section(FAVORITES));
    h.record(radio(RADIO_RECENT));
    h.record(tab(ALBUMS, None));
    h.back();
    h.back();
    assert_eq!(h.cursor(), 2, "standing on Favorites, with two entries still ahead");

    h.forget_section(NAV_RADIO);

    assert_eq!(h.len(), 3, "both Radio entries go");
    assert_eq!(h.cursor(), 1, "and the cursor comes back by the one that preceded it");
    assert_eq!(h.current(), Some(section(FAVORITES)), "still on the page the user is standing on");
    assert_eq!(h.back(), Some(section(BROWSE)));
    assert_eq!(h.back(), None);
    assert_eq!(h.forward(), Some(section(FAVORITES)));
    assert_eq!(h.forward(), Some(tab(ALBUMS, None)), "the forward stack skips what was dropped");
}

/// Entries ahead of the cursor cost it nothing: the forward stack shortens, the user's
/// own position does not move.
#[test]
fn forgetting_a_section_ahead_of_the_cursor_leaves_it_alone() {
    let mut h = NavHistory::new();
    h.record(section(BROWSE));
    h.record(radio(RADIO_BROWSE));
    h.back();
    assert_eq!(h.cursor(), 0);

    h.forget_section(NAV_RADIO);

    assert_eq!(h.len(), 1);
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.current(), Some(section(BROWSE)));
    assert_eq!(h.forward(), None, "the entry it could have walked to is gone");
}

/// The edge the `min` clamp is for: a history that was nothing but Radio empties, and
/// `entries.len() - 1` would underflow on the way to describing where the cursor lands.
#[test]
fn forgetting_the_only_section_present_empties_the_history() {
    let mut h = NavHistory::new();
    h.record(radio(RADIO_BROWSE));
    h.record(radio(RADIO_RECENT));

    h.forget_section(NAV_RADIO);

    assert!(h.is_empty());
    assert_eq!(h.cursor(), 0);
    assert_eq!(h.current(), None, "a cursor over an empty history describes nothing");
    assert_eq!(h.back(), None);
    assert_eq!(h.forward(), None);
}

#[test]
fn forgetting_a_section_that_never_recorded_changes_nothing() {
    let mut h = NavHistory::new();
    h.record(section(BROWSE));
    h.record(tab(ALBUMS, Some(7)));

    h.forget_section(NAV_RADIO);

    assert_eq!(h.len(), 2);
    assert_eq!(h.cursor(), 1);
    assert_eq!(h.current(), Some(tab(ALBUMS, Some(7))));
}
