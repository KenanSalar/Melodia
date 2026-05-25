use std::path::PathBuf;

use crate::media::watcher::FileEvent;

use super::deduplicate_events;

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
