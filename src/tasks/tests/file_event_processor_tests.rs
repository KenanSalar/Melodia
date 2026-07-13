use std::path::{Path, PathBuf};

use crate::media::self_writes::SelfWrites;
use crate::media::watcher::FileEvent;

use super::{deduplicate_events, suppress_self_writes};

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

    let batch = vec![
        FileEvent::Modified(PathBuf::from("/music/edited.mp3")),
        FileEvent::Modified(PathBuf::from("/music/external.mp3")),
    ];

    let result = suppress_self_writes(batch, &self_writes);
    assert_eq!(
        result,
        vec![FileEvent::Modified(PathBuf::from("/music/external.mp3"))]
    );
}

#[test]
fn suppress_only_filters_modified() {
    // A tag write rewrites the file in place, so it can only ever produce a
    // Modified. Anything else for the same path is a genuine external change
    // and must pass through even while the path is marked.
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let batch = vec![
        FileEvent::Created(PathBuf::from("/music/edited.mp3")),
        FileEvent::Removed(PathBuf::from("/music/edited.mp3")),
        FileEvent::Renamed {
            from: PathBuf::from("/music/edited.mp3"),
            to: PathBuf::from("/music/moved.mp3"),
        },
    ];

    let result = suppress_self_writes(batch.clone(), &self_writes);
    assert_eq!(result, batch);
}

#[test]
fn suppress_consumes_the_mark_so_a_second_event_survives() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/edited.mp3"));

    let echo = vec![FileEvent::Modified(PathBuf::from("/music/edited.mp3"))];
    assert!(suppress_self_writes(echo.clone(), &self_writes).is_empty());

    // A later, genuinely external edit to the same file is not swallowed.
    assert_eq!(suppress_self_writes(echo.clone(), &self_writes), echo);
}

#[test]
fn suppress_can_empty_the_batch() {
    let self_writes = SelfWrites::default();
    self_writes.mark(Path::new("/music/a.mp3"));
    self_writes.mark(Path::new("/music/b.mp3"));

    let batch = vec![
        FileEvent::Modified(PathBuf::from("/music/a.mp3")),
        FileEvent::Modified(PathBuf::from("/music/b.mp3")),
    ];

    assert!(suppress_self_writes(batch, &self_writes).is_empty());
}
