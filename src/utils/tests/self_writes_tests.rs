//! Self-write suppression-set tests.
//!
//! The TTL cases go through the private `mark_at` / `take_recent_at` halves so
//! expiry is exercised without sleeping for `SELF_WRITE_TTL`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{MIN_SWEEP_AT, SELF_WRITE_TTL, SelfWrites};

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
fn an_expired_entry_is_dropped_on_lookup_without_a_sweep() {
    // Below the sweep threshold nothing is swept, so the TTL check inside
    // `take_recent_at` is the only thing standing between a stale mark and a
    // swallowed external edit. It must both reject the entry and consume it.
    let writes = SelfWrites::default();
    let start = Instant::now();
    let past_the_ttl = start + SELF_WRITE_TTL + Duration::from_secs(1);

    writes.mark_at(Path::new("/music/errored.mp3"), start);
    writes.mark_at(Path::new("/music/fresh.mp3"), past_the_ttl);
    assert_eq!(writes.pending(), 2);

    // The expired mark doesn't suppress, and looking it up clears it.
    assert!(!writes.take_recent_at(Path::new("/music/errored.mp3"), past_the_ttl));
    assert_eq!(writes.pending(), 1);

    // An unrelated lookup leaves the fresh entry alone, and it still suppresses.
    assert!(!writes.take_recent_at(Path::new("/music/unrelated.mp3"), past_the_ttl));
    assert!(writes.take_recent_at(Path::new("/music/fresh.mp3"), past_the_ttl));
    assert_eq!(writes.pending(), 0);
}

#[test]
fn marking_sweeps_once_the_map_outgrows_the_threshold() {
    // A batch edit with folder watching off fires no events at all, so the read
    // path never runs and can't be what bounds the map. The insert path has to.
    let writes = SelfWrites::default();
    let start = Instant::now();
    let past_the_ttl = start + SELF_WRITE_TTL + Duration::from_secs(1);

    for i in 0..MIN_SWEEP_AT {
        writes.mark_at(Path::new(&format!("/music/stale-{i}.mp3")), start);
    }
    // Still below the threshold, so nothing has been swept yet.
    assert_eq!(writes.pending(), MIN_SWEEP_AT);

    // The next mark crosses it. Every prior entry is past the TTL by now, so the
    // sweep collapses the map to just this one — no `take_recent` involved.
    writes.mark_at(Path::new("/music/fresh.mp3"), past_the_ttl);
    assert_eq!(writes.pending(), 1);
    assert!(writes.take_recent_at(Path::new("/music/fresh.mp3"), past_the_ttl));
}
