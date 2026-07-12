use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{
    new_debouncer, DebounceEventResult, Debouncer, RecommendedCache,
};
use tokio::sync::mpsc;

use crate::error::AppError;
use crate::media::is_audio_extension;

/// Classified file system event sent through the mpsc channel to the event processor.
#[derive(Debug, Clone, PartialEq)]
pub enum FileEvent {
    Created(PathBuf),
    Removed(PathBuf),
    Modified(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    /// Kernel queue overflow (Linux inotify `IN_Q_OVERFLOW` or platform
    /// equivalent) — surfaced by `notify` via `event.need_rescan()`. The
    /// per-file event stream is no longer trustworthy; consumer should
    /// trigger a full library reconcile instead of acting on individual
    /// events in the same batch.
    RescanNeeded,
}

pub struct FolderWatcher {
    debouncer: Option<Debouncer<notify::RecommendedWatcher, RecommendedCache>>,
    tx: mpsc::Sender<FileEvent>,
}

impl FolderWatcher {
    pub fn new(tx: mpsc::Sender<FileEvent>) -> Self {
        Self {
            debouncer: None,
            tx,
        }
    }

    pub fn start(&mut self, paths: &[PathBuf]) -> Result<(), AppError> {
        self.stop();

        let tx = self.tx.clone();

        let mut debouncer = new_debouncer(
            Duration::from_secs(2),
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        // Any rescan flag in the batch means the kernel dropped events
                        // somewhere — the per-file events around it can't be trusted.
                        // Send a single RescanNeeded and skip classification; the
                        // processor reconciles the whole library against disk.
                        if events.iter().any(|e| e.need_rescan()) {
                            log::warn!(
                                "Watcher queue overflow flag set — requesting full library rescan"
                            );
                            if let Err(e) = tx.blocking_send(FileEvent::RescanNeeded) {
                                log::warn!("File event channel closed (shutdown?): {e}");
                            }
                            return;
                        }
                        for event in events {
                            for file_event in classify_event(event.kind, &event.paths) {
                                // blocking_send back-pressures the debouncer thread when the
                                // consumer is slow rather than silently dropping events;
                                // failure means the receiver was dropped (i.e. shutdown).
                                if let Err(e) = tx.blocking_send(file_event) {
                                    log::warn!(
                                        "File event channel closed (shutdown?): {e}"
                                    );
                                }
                            }
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            log::warn!("Watch error: {error:?}");
                        }
                    }
                }
            },
        )
        .map_err(|e| AppError::watcher("Failed to create watcher", e))?;

        for path in paths {
            if path.exists() {
                if let Err(e) = debouncer.watch(path, RecursiveMode::Recursive) {
                    log::warn!("Failed to watch {}: {}", path.display(), e);
                } else {
                    log::info!("Watching folder: {}", path.display());
                }
            }
        }

        self.debouncer = Some(debouncer);
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.debouncer.take().is_some() {
            log::info!("Folder watcher stopped");
        }
    }
}

/// Classify a notify event into zero or more `FileEvent`s.
/// Filters out non-audio files.
fn classify_event(kind: EventKind, paths: &[PathBuf]) -> Vec<FileEvent> {
    let mut events = Vec::new();

    match kind {
        // Cross-directory rename surfaces as a `To` event on the destination side
        // with no matching `From`, indistinguishable from a fresh create.
        EventKind::Create(CreateKind::File | CreateKind::Any)
        | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            for path in paths {
                if is_audio_file(path) {
                    events.push(FileEvent::Created(path.clone()));
                }
            }
        }
        // Cross-directory rename source side: only one path visible.
        EventKind::Remove(RemoveKind::File | RemoveKind::Any)
        | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            for path in paths {
                if is_audio_file(path) {
                    events.push(FileEvent::Removed(path.clone()));
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            if paths.len() >= 2 {
                let from = &paths[0];
                let to = &paths[1];
                let from_audio = is_audio_file(from);
                let to_audio = is_audio_file(to);
                if from_audio && to_audio {
                    events.push(FileEvent::Renamed {
                        from: from.clone(),
                        to: to.clone(),
                    });
                } else if from_audio {
                    // audio → non-audio: treat as removal
                    events.push(FileEvent::Removed(from.clone()));
                } else if to_audio {
                    // non-audio → audio: treat as creation
                    events.push(FileEvent::Created(to.clone()));
                }
            }
        }
        EventKind::Modify(
            ModifyKind::Data(_) | ModifyKind::Any | ModifyKind::Name(RenameMode::Other),
        ) => {
            for path in paths {
                if is_audio_file(path) {
                    events.push(FileEvent::Modified(path.clone()));
                }
            }
        }
        // Access, Metadata, Other — skip
        _ => {}
    }

    events
}

pub(crate) fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_audio_extension)
}

#[cfg(test)]
#[path = "tests/watcher_tests.rs"]
mod tests;
