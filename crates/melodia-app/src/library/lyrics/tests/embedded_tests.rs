//! Which of a tag's lyrics entries shows, in the panel and in Edit Tags alike.

use super::*;

const PLAIN: &str = "words nothing follows";
const LINES: &str = "[00:01.00]a line with a stamp";
const WORDS: &str = "[00:01.00]<00:01.00>a <00:01.50>word";
/// Text nothing draws, as a tag of nothing but metadata is.
const UNREAD: &str = "[ti:Song]";

fn shown(entries: &[&str]) -> Option<String> {
    let entries: Vec<String> = entries.iter().map(|entry| (*entry).to_owned()).collect();
    best_tag_entry(&entries).map(str::to_owned)
}

#[test]
fn a_timed_entry_behind_a_plain_one_is_the_one_shown() {
    // Taking the first, the panel shows prose for a file that carries a timed copy too.
    assert_eq!(shown(&[PLAIN, LINES]).as_deref(), Some(LINES));
}

#[test]
fn a_plain_entry_behind_a_timed_one_stays_behind_it() {
    assert_eq!(shown(&[LINES, PLAIN]).as_deref(), Some(LINES));
}

#[test]
fn word_times_outrank_line_times() {
    assert_eq!(shown(&[LINES, WORDS]).as_deref(), Some(WORDS));
}

#[test]
fn of_two_entries_timed_alike_the_first_in_the_tag_is_shown() {
    assert_eq!(shown(&[LINES, "[00:02.00]another upload"]).as_deref(), Some(LINES));
}

#[test]
fn an_entry_nothing_parses_from_ranks_below_plain_text() {
    assert_eq!(shown(&[UNREAD, PLAIN]).as_deref(), Some(PLAIN));
}

#[test]
fn an_entry_nothing_parses_from_is_still_the_one_offered_when_it_is_all_there_is() {
    // The editor hands back what the file holds; that the panel draws nothing of it is the
    // panel's answer.
    assert_eq!(shown(&[UNREAD]).as_deref(), Some(UNREAD));
}

#[test]
fn a_tag_with_no_entries_shows_nothing() {
    assert_eq!(shown(&[]), None);
}
