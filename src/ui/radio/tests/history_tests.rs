//! The ring behind the session song history.
//!
//! [`super::StationHistory::note`] is folded per view-model tick, so its three refusals are what
//! keep the list a list of songs rather than of metadata blocks — and each is invisible from the
//! surfaces that draw it, both of which simply paint whatever the ring holds.

use super::{HISTORY_CAP, StationHistory};

const A: &str = "https://stream.example/a";
const B: &str = "https://stream.example/b";

fn titles(history: &StationHistory) -> Vec<&str> {
    history.titles().iter().map(String::as_str).collect()
}

#[test]
fn titles_accumulate_newest_first() {
    let mut history = StationHistory::default();
    assert!(history.note(A, Some("First")));
    assert!(history.note(A, Some("Second")));
    assert_eq!(titles(&history), ["Second", "First"]);
}

/// **A station change clears the ring, and playback stopping does not.** The list belongs to the
/// station on screen; a station you paused is still that station, so the titles have to survive a
/// tick that carries no title at all.
#[test]
fn a_different_station_clears_what_the_last_one_announced() {
    let mut history = StationHistory::default();
    history.note(A, Some("First"));
    history.note(A, Some("Second"));

    assert!(history.note(B, None), "the clear itself is a move, even with nothing to put back");
    assert!(titles(&history).is_empty());
    assert!(history.describes(B));
    assert!(!history.describes(A));
}

#[test]
fn a_tick_with_no_title_keeps_the_station_and_its_titles() {
    let mut history = StationHistory::default();
    history.note(A, Some("First"));

    assert!(!history.note(A, None), "nothing moved, so nothing is republished");
    assert_eq!(titles(&history), ["First"]);
    assert!(history.describes(A));
}

/// Stations re-send the current title on a timer, and plenty send it verbatim with every metadata
/// block — so without the dedupe the list is one song repeated for as long as it plays.
#[test]
fn a_title_repeated_on_the_stations_timer_lands_once() {
    let mut history = StationHistory::default();
    assert!(history.note(A, Some("Same Song")));
    for _ in 0..8 {
        assert!(!history.note(A, Some("Same Song")), "a repeat is not a move");
    }
    assert_eq!(titles(&history), ["Same Song"]);

    // Only against the newest, so a song that comes round again later is a new entry.
    history.note(A, Some("Another"));
    assert!(history.note(A, Some("Same Song")));
    assert_eq!(titles(&history), ["Same Song", "Another", "Same Song"]);
}

#[test]
fn a_blank_or_whitespace_title_is_not_a_song() {
    let mut history = StationHistory::default();
    for blank in ["", "   ", "\t", "\n"] {
        assert!(!history.note(A, Some(blank)), "{blank:?} is not something to list");
    }
    assert!(titles(&history).is_empty());

    assert!(history.note(A, Some("  Padded  ")));
    assert_eq!(titles(&history), ["Padded"], "and a real title arrives trimmed");
}

/// A stream can be left running for days, so the ring is bounded and drops from the far end.
#[test]
fn the_ring_holds_its_cap_and_drops_the_oldest() {
    let mut history = StationHistory::default();
    for n in 0..HISTORY_CAP + 10 {
        history.note(A, Some(&format!("Song {n}")));
    }

    let held = titles(&history);
    assert_eq!(held.len(), HISTORY_CAP);
    assert_eq!(held.first().copied(), Some(format!("Song {}", HISTORY_CAP + 9)).as_deref());
    assert_eq!(held.last().copied(), Some("Song 10"), "the oldest ten fell off the end");
}
