use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{
    Config, Event, EventHandler, EventKind, PathsMut, RecommendedWatcher, RecursiveMode, Watcher,
    WatcherKind,
};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer_opt};
use tokio::sync::mpsc;

use melodia_core::error::{AppError, describe};
use melodia_core::utils::audio_ext::is_audio_extension;

/// Classified file system event sent through the mpsc channel to the event processor.
#[derive(Debug, Clone, PartialEq)]
pub enum FileEvent {
    Created(PathBuf),
    Removed(PathBuf),
    Modified(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// The kernel's queue overflowed, so the batch's per-file events can't be trusted and the
    /// library is reconciled whole.
    RescanNeeded,
}

/// The recommended watcher, minus the access events nothing here reads.
///
/// notify's inotify backend watches `IN_OPEN` unconditionally, so every walk, scan, hash and
/// playback open raises an event on each watch that sees it; dropped here, they stay out of the
/// debouncer's queues too. Goes once a stable notify carries notify-rs/notify#736's kernel-side
/// filter.
struct ChangeWatcher(RecommendedWatcher);

/// Hands every event but an access on to the debouncer, rescan flags and errors included.
struct SkipAccess<H>(H);

impl<H: EventHandler> EventHandler for SkipAccess<H> {
    fn handle_event(&mut self, event: notify::Result<Event>) {
        if matches!(&event, Ok(event) if event.kind.is_access() && !event.need_rescan()) {
            return;
        }
        self.0.handle_event(event);
    }
}

impl Watcher for ChangeWatcher {
    fn new<F: EventHandler>(event_handler: F, config: Config) -> notify::Result<Self> {
        RecommendedWatcher::new(SkipAccess(event_handler), config).map(Self)
    }

    fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> notify::Result<()> {
        self.0.watch(path, recursive_mode)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        self.0.unwatch(path)
    }

    fn paths_mut<'me>(&'me mut self) -> Box<dyn PathsMut + 'me> {
        self.0.paths_mut()
    }

    fn configure(&mut self, option: Config) -> notify::Result<bool> {
        self.0.configure(option)
    }

    fn kind() -> WatcherKind {
        RecommendedWatcher::kind()
    }
}

pub struct FolderWatcher {
    debouncer: Option<Debouncer<ChangeWatcher, RecommendedCache>>,
    /// Roots the running debouncer holds a recursive watch on.
    roots: Vec<PathBuf>,
    tx: mpsc::Sender<FileEvent>,
}

impl FolderWatcher {
    pub fn new(tx: mpsc::Sender<FileEvent>) -> Self {
        Self { debouncer: None, roots: Vec::new(), tx }
    }

    pub fn start(&mut self, paths: &[PathBuf]) -> Result<(), AppError> {
        self.stop();

        let tx = self.tx.clone();

        let debouncer = new_debouncer_opt::<_, ChangeWatcher, _>(
            Duration::from_secs(2),
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        // A rescan flag means the kernel dropped events, so none around it can
                        // be trusted.
                        if events.iter().any(|e| e.need_rescan()) {
                            log::warn!(
                                "Watcher queue overflow flag set — requesting full library rescan"
                            );
                            if let Err(e) = tx.blocking_send(FileEvent::RescanNeeded) {
                                log::warn!(
                                    "File event channel closed (shutdown?): {}",
                                    describe(&e)
                                );
                            }
                            return;
                        }
                        for event in events {
                            for file_event in classify_event(event.kind, &event.paths) {
                                // Back-pressures the debouncer rather than dropping events, and
                                // fails only once the receiver is gone at shutdown.
                                if let Err(e) = tx.blocking_send(file_event) {
                                    log::warn!(
                                        "File event channel closed (shutdown?): {}",
                                        describe(&e)
                                    );
                                }
                            }
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            log::warn!("Watch error: {}", describe(&error));
                        }
                    }
                }
            },
            RecommendedCache::new(),
            Config::default(),
        )
        .map_err(|e| AppError::watcher("Failed to create watcher", e))?;

        self.debouncer = Some(debouncer);
        self.retarget(paths);
        Ok(())
    }

    /// Moves a running watcher onto `paths` by the difference, so a root both lists share stays
    /// watched throughout. A stopped watcher stays stopped.
    pub fn retarget(&mut self, paths: &[PathBuf]) {
        let Some(debouncer) = self.debouncer.as_mut() else {
            return;
        };

        // Dropped roots go first: a parent replacing its children registers their directories
        // again, and unwatching a child after that would take those watches with it.
        self.roots.retain(|root| {
            if paths.contains(root) {
                return true;
            }
            // A root whose directory vanished has already lost its watch.
            match debouncer.unwatch(root) {
                Ok(()) => log::info!("Stopped watching folder: {}", root.display()),
                Err(e) => log::debug!("Failed to unwatch {}: {}", root.display(), describe(&e)),
            }
            false
        });

        for path in paths {
            if self.roots.contains(path) || !path.exists() {
                continue;
            }
            match debouncer.watch(path, RecursiveMode::Recursive) {
                Ok(()) => {
                    log::info!("Watching folder: {}", path.display());
                    self.roots.push(path.clone());
                }
                Err(e) => log::warn!("Failed to watch {}: {}", path.display(), describe(&e)),
            }
        }
    }

    pub fn stop(&mut self) {
        self.roots.clear();
        if self.debouncer.take().is_some() {
            log::info!("Folder watcher stopped");
        }
    }
}

/// The `FileEvent`s a notify event means for audio files.
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
                    events.push(FileEvent::Renamed { from: from.clone(), to: to.clone() });
                } else if from_audio {
                    events.push(FileEvent::Removed(from.clone()));
                } else if to_audio {
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
        _ => {}
    }

    events
}

pub(crate) fn is_audio_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(is_audio_extension)
}

#[cfg(test)]
#[path = "tests/watcher_tests.rs"]
mod tests;
