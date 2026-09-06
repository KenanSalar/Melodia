use std::path::{Path, PathBuf};

use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::media::ingest::watcher::FileEvent;

use super::{BatchPlan, deduplicate_events, plan_batch, suppress_self_writes};

#[test]
fn dedup_created_then_removed_cancels() {
    let events = vec![
        FileEvent::Created(PathBuf::from("/music/song.mp3")),
        FileEvent::Removed(PathBuf::from("/music/song.mp3")),
    ];
    let result = deduplicate_events(events);
    assert!(result.is_empty());
}

#[test]
fn dedup_removed_then_created_becomes_modified() {
    let events = vec![
        FileEvent::Removed(PathBuf::from("/music/song.mp3")),
        FileEvent::Created(PathBuf::from("/music/song.mp3")),
    ];
    let result = deduplicate_events(events);
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], FileEvent::Modified(_)));
}

#[test]
fn dedup_created_then_modified_keeps_created() {
    let events = vec![
        FileEvent::Created(PathBuf::from("/music/song.mp3")),
        FileEvent::Modified(PathBuf::from("/music/song.mp3")),
    ];
    let result = deduplicate_events(events);
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], FileEvent::Created(_)));
}

#[test]
fn dedup_multiple_modified_keeps_one() {
    let events = vec![
        FileEvent::Modified(PathBuf::from("/music/song.mp3")),
        FileEvent::Modified(PathBuf::from("/music/song.mp3")),
        FileEvent::Modified(PathBuf::from("/music/song.mp3")),
    ];
    let result = deduplicate_events(events);
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], FileEvent::Modified(_)));
}

#[test]
fn dedup_different_paths_preserved() {
    let events = vec![
        FileEvent::Created(PathBuf::from("/music/a.mp3")),
        FileEvent::Created(PathBuf::from("/music/b.mp3")),
        FileEvent::Removed(PathBuf::from("/music/c.mp3")),
    ];
    let result = deduplicate_events(events);
    assert_eq!(result.len(), 3);
}

#[test]
fn dedup_rename_clears_prior_events_for_both_paths() {
    let events = vec![
        FileEvent::Modified(PathBuf::from("/music/old.mp3")),
        FileEvent::Modified(PathBuf::from("/music/new.mp3")),
        FileEvent::Renamed {
            from: PathBuf::from("/music/old.mp3"),
            to: PathBuf::from("/music/new.mp3"),
        },
    ];
    let result = deduplicate_events(events);
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], FileEvent::Renamed { .. }));
}

#[test]
fn dedup_rename_then_modified_keeps_rename() {
    let events = vec![
        FileEvent::Renamed {
            from: PathBuf::from("/music/old.mp3"),
            to: PathBuf::from("/music/new.mp3"),
        },
        FileEvent::Modified(PathBuf::from("/music/new.mp3")),
    ];
    let result = deduplicate_events(events);
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], FileEvent::Renamed { .. }));
}

#[test]
fn dedup_empty_returns_empty() {
    let events: Vec<FileEvent> = vec![];
    let result = deduplicate_events(events);
    assert!(result.is_empty());
}

#[test]
fn dedup_rescan_supersedes_everything_in_batch() {
    // Kernel queue overflow flag invalidates the per-file events in the
    // same batch; processor must reconcile against disk rather than act
    // on a truncated stream.
    let events = vec![
        FileEvent::Created(PathBuf::from("/music/a.mp3")),
        FileEvent::Modified(PathBuf::from("/music/b.mp3")),
        FileEvent::RescanNeeded,
        FileEvent::Removed(PathBuf::from("/music/c.mp3")),
    ];
    let result = deduplicate_events(events);
    assert_eq!(result, vec![FileEvent::RescanNeeded]);
}

#[test]
fn suppress_drops_modified_events_for_paths_we_wrote() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let mut batch = vec![
        FileEvent::Modified(PathBuf::from("/music/edited.mp3")),
        FileEvent::Modified(PathBuf::from("/music/external.mp3")),
    ];

    suppress_self_writes(&mut batch, &self_writes);
    assert_eq!(batch, vec![FileEvent::Modified(PathBuf::from("/music/external.mp3"))]);
}

#[test]
fn suppress_only_filters_modified() {
    // A tag write rewrites the file in place, so it can only ever produce a
    // Modified. Anything else for the same path is a genuine external change
    // and must pass through even while the path is marked.
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let original = vec![
        FileEvent::Created(PathBuf::from("/music/edited.mp3")),
        FileEvent::Removed(PathBuf::from("/music/edited.mp3")),
        FileEvent::Renamed {
            from: PathBuf::from("/music/edited.mp3"),
            to: PathBuf::from("/music/moved.mp3"),
        },
    ];

    let mut batch = original.clone();
    suppress_self_writes(&mut batch, &self_writes);
    assert_eq!(batch, original);
}

#[test]
fn suppress_consumes_the_mark_so_a_second_event_survives() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let echo = vec![FileEvent::Modified(PathBuf::from("/music/edited.mp3"))];

    let mut batch = echo.clone();
    suppress_self_writes(&mut batch, &self_writes);
    assert!(batch.is_empty());

    // A later, genuinely external edit to the same file is not swallowed.
    let mut batch = echo.clone();
    suppress_self_writes(&mut batch, &self_writes);
    assert_eq!(batch, echo);
}

#[test]
fn suppress_can_empty_the_batch() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/a.mp3"));
    self_writes.mark(Path::new("/music/b.mp3"));

    let mut batch = vec![
        FileEvent::Modified(PathBuf::from("/music/a.mp3")),
        FileEvent::Modified(PathBuf::from("/music/b.mp3")),
    ];

    suppress_self_writes(&mut batch, &self_writes);
    assert!(batch.is_empty());
}

// ---- the order the two filters are applied in ----

/// The events a plan would apply, or `None` for the two arms that apply none.
fn planned(plan: BatchPlan) -> Option<Vec<FileEvent>> {
    match plan {
        BatchPlan::Process(batch) => Some(batch),
        BatchPlan::Nothing | BatchPlan::Rescan => None,
    }
}

/// `process_batch` `unreachable!()`s on a `RescanNeeded`, so the flag has to win from anywhere
/// in a batch rather than only from a batch of one. `deduplicate_events` collapses such a batch
/// today, which is exactly what would let a slice-pattern version of the check look correct and
/// panic a background task the first time dedup changed.
#[test]
fn a_rescan_wins_from_a_batch_it_does_not_have_to_itself() {
    let self_writes = SelfWrites::default();

    let batch = vec![
        FileEvent::Modified(PathBuf::from("/music/a.mp3")),
        FileEvent::RescanNeeded,
        FileEvent::Created(PathBuf::from("/music/b.mp3")),
    ];

    assert!(
        matches!(plan_batch(batch, &self_writes), BatchPlan::Rescan),
        "a rescan routed into process_batch is a panic, not a slow path"
    );
}

/// A rescan re-derives every enabled folder from disk and consults no marks, and `take_recent`
/// consumes: spending one here would leave the write's real echo unsuppressed for the rest of
/// its TTL. `SelfWrites`' own header states this ordering as a settled trade and cannot see the
/// code that keeps it true.
#[test]
fn a_rescan_does_not_spend_the_marks_it_never_needed() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let batch = vec![
        FileEvent::RescanNeeded,
        FileEvent::Modified(PathBuf::from("/music/edited.mp3")),
    ];
    assert!(matches!(plan_batch(batch, &self_writes), BatchPlan::Rescan));

    assert!(
        self_writes.take_recent(Path::new("/music/edited.mp3")),
        "the mark has to survive to suppress the echo when it actually arrives"
    );
}

/// Suppression runs before `process_batch`, not after, which is what keeps the re-hash, the
/// lofty re-parse and the artwork re-extract off a file we rewrote seconds earlier.
#[test]
fn a_mixed_batch_applies_only_the_events_we_did_not_cause() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let batch = vec![
        FileEvent::Modified(PathBuf::from("/music/edited.mp3")),
        FileEvent::Modified(PathBuf::from("/music/external.mp3")),
    ];

    assert_eq!(
        planned(plan_batch(batch, &self_writes)),
        Some(vec![FileEvent::Modified(PathBuf::from("/music/external.mp3"))])
    );
}

/// A batch that suppression empties asks for nothing rather than for an empty transaction: the
/// bump on the far side of `process_batch` makes every open list re-fetch, and a tag write
/// already refreshed the row it would be re-fetching.
#[test]
fn a_batch_of_only_our_own_writes_asks_for_nothing() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let batch = vec![FileEvent::Modified(PathBuf::from("/music/edited.mp3"))];

    assert!(matches!(plan_batch(batch, &self_writes), BatchPlan::Nothing));
}

/// An empty batch is the arm the drain loop reaches when dedup cancels everything in it, a
/// create and a remove for one path inside one window being the ordinary way that happens.
#[test]
fn an_empty_batch_asks_for_nothing() {
    let self_writes = SelfWrites::default();

    assert!(matches!(plan_batch(Vec::new(), &self_writes), BatchPlan::Nothing));
}
