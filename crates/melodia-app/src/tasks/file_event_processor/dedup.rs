//! Watcher-event deduplication within a 500 ms batching window.

use std::collections::HashMap;
use std::path::PathBuf;

use melodia_store::media::ingest::watcher::FileEvent;

/// Deduplicate events within a batch using priority rules:
/// - Created + Removed for same path → cancel both
/// - Created + Modified for same path → keep only Created
/// - Multiple Modified for same path → keep only one
/// - Renamed target + Modified → keep only Renamed
///
/// The map stores `DedupKind` (no path) keyed by `PathBuf`, so each
/// event's path moves into the map exactly once instead of being cloned
/// twice (once for the key, once into the stored event). For `Renamed`
/// the `from` path lives inside the kind variant; the `to` path is the
/// map key.
pub(super) fn deduplicate_events(events: Vec<FileEvent>) -> Vec<FileEvent> {
    enum DedupKind {
        Created,
        Removed,
        Modified,
        Renamed { from: PathBuf },
    }

    // Kernel told us it dropped events somewhere in this batch — any per-file
    // event we collected is potentially stale. Collapse the whole batch to a
    // single RescanNeeded so the caller reconciles against disk instead.
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
