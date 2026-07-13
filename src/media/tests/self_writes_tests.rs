//! Self-write suppression-set tests.
//!
//! The TTL cases go through the private `mark_at` / `take_recent_at` halves so
//! expiry is exercised without sleeping for `SELF_WRITE_TTL`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{SELF_WRITE_TTL, SelfWrites};

#[test]
fn an_unmarked_path_is_not_recent() {
    let writes = SelfWrites::default();
    assert!(!writes.take_recent(Path::new("/music/song.mp3")));
}

#[test]
fn a_marked_path_is_recent_once_and_then_consumed() {
    let writes = SelfWrites::default();
    let path = Path::new("/music/song.mp3");

    writes.mark(path);
    assert!(writes.take_recent(path));
    assert!(!writes.take_recent(path));
    assert_eq!(writes.pending(), 0);
}

#[test]
fn marking_one_path_does_not_suppress_another() {
    let writes = SelfWrites::default();
    writes.mark(Path::new("/music/a.mp3"));

    assert!(!writes.take_recent(Path::new("/music/b.mp3")));
    assert!(writes.take_recent(Path::new("/music/a.mp3")));
}

#[test]
fn unmark_drops_the_entry() {
    let writes = SelfWrites::default();
    let path = Path::new("/music/song.mp3");

    writes.mark(path);
    writes.unmark(&[PathBuf::from("/music/song.mp3")]);

    assert_eq!(writes.pending(), 0);
    assert!(!writes.take_recent(path));
}

#[test]
fn an_expired_entry_is_not_recent() {
    let writes = SelfWrites::default();
    let path = Path::new("/music/song.mp3");

    // Age the entry by moving the *lookup* forward rather than the mark back —
    // `Instant` subtraction is a clippy error under the pedantic gate, and its
    // suggested `checked_sub().unwrap()` is denied outright.
    let written_at = Instant::now();
    let past_the_ttl = written_at + SELF_WRITE_TTL + Duration::from_secs(1);

    writes.mark_at(path, written_at);

    assert!(!writes.take_recent_at(path, past_the_ttl));
}

#[test]
fn expired_entries_are_swept_by_an_unrelated_lookup() {
    // A write that errored never fires an event, so nothing ever consumes its
    // entry. The sweep on the read path is what stops it leaking.
    let writes = SelfWrites::default();
    let start = Instant::now();
    let now = start + SELF_WRITE_TTL + Duration::from_secs(1);

    writes.mark_at(Path::new("/music/errored-a.mp3"), start);
    writes.mark_at(Path::new("/music/errored-b.mp3"), start);
    writes.mark_at(Path::new("/music/fresh.mp3"), now);
    assert_eq!(writes.pending(), 3);

    assert!(!writes.take_recent_at(Path::new("/music/unrelated.mp3"), now));

    // Both stale entries are gone; the fresh one survives untouched.
    assert_eq!(writes.pending(), 1);
    assert!(writes.take_recent_at(Path::new("/music/fresh.mp3"), now));
}
