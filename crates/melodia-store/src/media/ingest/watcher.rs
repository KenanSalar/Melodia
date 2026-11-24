use std::mem;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{ErrorKind, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};
use tokio::sync::mpsc;
use walkdir::WalkDir;

use melodia_core::error::{AppError, describe};
use melodia_core::utils::audio_ext::{is_audio_extension, is_cue_sheet, is_library_extension};
use melodia_core::utils::toast::{self, ToastKind};
use melodia_core::utils::trash_dir::in_trash_dir;

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
    /// A directory holding audio was renamed or moved. The OS reports nothing for the files inside
    /// it, so the library is reconciled rather than leave their rows on the old paths.
    DirectoryRenamed,
    /// A path that reads as neither audio nor a cue sheet was removed or moved out of the watch,
    /// which may have been a directory: the debouncer folds a deleted directory's file removals
    /// into its own, and one moved out reports nothing for its files. The library is reconciled if
    /// it holds tracks under the path.
    DirectoryGone(PathBuf),
}

pub struct FolderWatcher {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
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

        // `new_debouncer`'s event mask keeps opens and closes out of inotify's queue, where every
        // walk, scan, hash and playback would otherwise add an event for each entry it opens.
        let debouncer =
            new_debouncer(Duration::from_secs(2), None, move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        for file_event in batch_events(&events) {
                            forward(&tx, file_event);
                        }
                    }
                    Err(errors) => {
                        for error in &errors {
                            log::warn!("Watch error: {}", describe(error));
                        }
                        for gap in first_of_each_kind(errors.iter().filter_map(watch_gap)) {
                            gap.report();
                        }
                    }
                }
            })
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

        // Dropped roots go first: unwatching a child drops the debouncer's file IDs beneath it
        // even under a parent still watching, and Windows and macOS pair a rename by those IDs.
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

        let mut gaps = Vec::new();
        for path in paths {
            if self.roots.contains(path) || !path.exists() {
                continue;
            }
            match debouncer.watch(path, RecursiveMode::Recursive) {
                Ok(()) => {
                    log::info!("Watching folder: {}", path.display());
                    self.roots.push(path.clone());
                }
                Err(e) => {
                    log::warn!("Failed to watch {}: {}", path.display(), describe(&e));
                    let Some(gap) = watch_gap(&e) else { continue };
                    // A walk the limit stopped keeps the watches it added, which only an unwatch
                    // releases.
                    if gap == WatchGap::Limit {
                        self.roots.push(path.clone());
                    }
                    gaps.push(gap);
                }
            }
        }
        for gap in first_of_each_kind(gaps) {
            gap.report();
        }
    }

    pub fn stop(&mut self) {
        self.roots.clear();
        if self.debouncer.take().is_some() {
            log::info!("Folder watcher stopped");
        }
    }
}

/// Back-pressures the debouncer rather than dropping the event. The send fails only once the
/// receiver is gone at shutdown.
fn forward(tx: &mpsc::Sender<FileEvent>, event: FileEvent) {
    if let Err(e) = tx.blocking_send(event) {
        log::warn!("File event channel closed (shutdown?): {}", describe(&e));
    }
}

/// What a watch error leaves unwatched, which the user otherwise learns of only when a change
/// never arrives.
#[derive(Debug, PartialEq)]
enum WatchGap {
    /// inotify ran out of watches, so every folder past the limit goes unwatched.
    Limit,
    Folder(PathBuf),
}

impl WatchGap {
    fn report(self) {
        match self {
            Self::Limit => toast::notify(ToastKind::WatchLimit, ""),
            Self::Folder(path) => {
                toast::notify(ToastKind::FolderUnwatched, path.display().to_string());
            }
        }
    }
}

/// The first gap of each kind: a walk reports every folder it can't enter, and the first sends the
/// user looking while the log names the rest.
fn first_of_each_kind(gaps: impl IntoIterator<Item = WatchGap>) -> Vec<WatchGap> {
    let mut firsts = Vec::new();
    for gap in gaps {
        let kind_seen =
            firsts.iter().any(|kept| mem::discriminant(kept) == mem::discriminant(&gap));
        if !kind_seen {
            firsts.push(gap);
        }
    }
    firsts
}

/// `None` where nothing is left out (a path gone before its watch, a watch already gone, a link
/// looping back up its tree) or the error names no folder for the user to act on.
fn watch_gap(error: &notify::Error) -> Option<WatchGap> {
    match error.kind {
        // The card names inotify's setting, where FSEvents caps roots by the open-file limit.
        ErrorKind::MaxFilesWatch if cfg!(target_os = "linux") => Some(WatchGap::Limit),
        // A walk's one generic error is that link loop.
        ErrorKind::PathNotFound | ErrorKind::WatchNotFound | ErrorKind::Generic(_) => None,
        _ => error.paths.first().cloned().map(WatchGap::Folder),
    }
}

/// The `FileEvent`s one debounced batch means for the library. A rescan flag anywhere in it means
/// the kernel dropped events, so none around it can be trusted.
fn batch_events(events: &[DebouncedEvent]) -> Vec<FileEvent> {
    if events.iter().any(|e| e.need_rescan()) {
        log::warn!("Watcher queue overflow flag set, requesting a full library rescan");
        return vec![FileEvent::RescanNeeded];
    }
    events.iter().flat_map(|event| classify_event(event.kind, &event.paths)).collect()
}

/// The `FileEvent`s a notify event means for the library's files, audio and cue sheets.
fn classify_event(kind: EventKind, paths: &[PathBuf]) -> Vec<FileEvent> {
    if relocates_an_audio_directory(kind, paths) {
        return vec![FileEvent::DirectoryRenamed];
    }

    let mut events = Vec::new();

    match kind {
        // Cross-directory rename surfaces as a `To` event on the destination side
        // with no matching `From`, indistinguishable from a fresh create.
        EventKind::Create(CreateKind::File | CreateKind::Any)
        | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            for path in paths {
                if is_library_file(path) {
                    events.push(FileEvent::Created(path.clone()));
                }
            }
        }
        // Nothing names the files a directory arrives with: Windows reports one moved in as this
        // alone, and inotify watches a new one only once its creation has been read.
        EventKind::Create(CreateKind::Folder) => {
            for dir in paths {
                events.extend(library_files_under(dir).map(FileEvent::Created));
            }
        }
        EventKind::Remove(RemoveKind::File) => {
            for path in paths {
                if is_library_file(path) {
                    events.push(FileEvent::Removed(path.clone()));
                }
            }
        }
        // Cross-directory rename source side: only one path visible.
        EventKind::Remove(RemoveKind::Folder | RemoveKind::Any)
        | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            events.extend(paths.iter().filter_map(|path| left_the_watch(path)));
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            if paths.len() >= 2 {
                let from = &paths[0];
                let to = &paths[1];
                let from_library = is_library_file(from);
                let to_library = is_library_file(to);
                if from_library && to_library {
                    events.push(FileEvent::Renamed { from: from.clone(), to: to.clone() });
                } else if from_library {
                    events.push(FileEvent::Removed(from.clone()));
                } else if to_library {
                    events.push(FileEvent::Created(to.clone()));
                }
            }
        }
        EventKind::Modify(
            ModifyKind::Data(_) | ModifyKind::Any | ModifyKind::Name(RenameMode::Other),
        ) => {
            for path in paths {
                if is_library_file(path) {
                    events.push(FileEvent::Modified(path.clone()));
                }
            }
        }
        _ => {}
    }

    events
}

/// Whether `kind` puts a directory with audio under it at a new path, read off the side that still
/// exists.
///
/// `Any` is a rename the debouncer couldn't pair, a case-only one on macOS, and `To` one from
/// outside the watch on Linux. `Create(Any)` is an entry Windows failed to stat or macOS sent
/// untyped, a move or a copy alike. A directory with no audio is passed over, or a drive-root
/// library would walk itself for every folder any app renames.
fn relocates_an_audio_directory(kind: EventKind, paths: &[PathBuf]) -> bool {
    let new_side = match kind {
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => paths.get(1),
        EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Any))
        | EventKind::Create(CreateKind::Any) => paths.first(),
        _ => None,
    };
    new_side.is_some_and(|dir| dir.is_dir() && holds_audio(dir))
}

fn holds_audio(dir: &Path) -> bool {
    WalkDir::new(dir).into_iter().flatten().any(|entry| is_audio_file(entry.path()))
}

fn library_files_under(dir: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file() && is_library_file(entry.path()))
        .map(walkdir::DirEntry::into_path)
}

/// A path gone from the watch, read by its name alone since it no longer exists: a removed track or
/// sheet if it reads as one, else a directory that may have held some.
fn left_the_watch(path: &Path) -> Option<FileEvent> {
    if is_library_file(path) {
        return Some(FileEvent::Removed(path.to_path_buf()));
    }
    (!in_trash_dir(path)).then(|| FileEvent::DirectoryGone(path.to_path_buf()))
}

/// Audio or a cue sheet outside every trash folder: a trashed file reads as removed, a restored one
/// as created.
fn is_library_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str());
    ext.is_some_and(is_library_extension) && !in_trash_dir(path)
}

pub(crate) fn is_audio_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(is_audio_extension)
}

pub fn is_sheet_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(is_cue_sheet)
}

#[cfg(test)]
#[path = "tests/watcher_tests.rs"]
mod tests;
