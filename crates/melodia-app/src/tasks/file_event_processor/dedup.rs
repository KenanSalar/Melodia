//! Watcher-event deduplication within a 500 ms batching window.

use std::collections::HashMap;
use std::path::PathBuf;

use melodia_store::media::ingest::watcher::FileEvent;

/// Collapses a batch to one event per path: a create then a remove cancel, a remove then a create
/// is a modify, and a create or a rename absorbs any modify. The path is the map key, so it moves
/// in once rather than being cloned for a key and a stored event.
pub(super) fn deduplicate_events(events: Vec<FileEvent>) -> Vec<FileEvent> {
    enum DedupKind {
        Created,
        Removed,
        Modified,
        Renamed { from: PathBuf },
    }

    // An overflow makes every per-file event suspect, so the caller reconciles against disk.
    if events.iter().any(|e| matches!(e, FileEvent::RescanNeeded)) {
        return vec![FileEvent::RescanNeeded];
    }

    let mut state: HashMap<PathBuf, DedupKind> = HashMap::with_capacity(events.len());

    for event in events {
        match event {
            FileEvent::Created(path) => {
                let kind = match state.get(&path) {
                    Some(DedupKind::Removed) => DedupKind::Modified,
                    _ => DedupKind::Created,
                };
                state.insert(path, kind);
            }
            FileEvent::Removed(path) => match state.get(&path) {
                Some(DedupKind::Created) => {
                    state.remove(&path);
                }
                _ => {
                    state.insert(path, DedupKind::Removed);
                }
            },
            FileEvent::Modified(path) => match state.get(&path) {
                Some(DedupKind::Created | DedupKind::Renamed { .. }) => {}
                _ => {
                    state.insert(path, DedupKind::Modified);
                }
            },
            FileEvent::Renamed { from, to } => {
                state.remove(&from);
                state.remove(&to);
                state.insert(to, DedupKind::Renamed { from });
            }
            // Filtered above; deduplicate_events early-returns on RescanNeeded.
            FileEvent::RescanNeeded => unreachable!(),
        }
    }

    state
        .into_iter()
        .map(|(path, kind)| match kind {
            DedupKind::Created => FileEvent::Created(path),
            DedupKind::Removed => FileEvent::Removed(path),
            DedupKind::Modified => FileEvent::Modified(path),
            DedupKind::Renamed { from } => FileEvent::Renamed { from, to: path },
        })
        .collect()
}
