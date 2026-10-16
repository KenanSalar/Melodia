use std::path::{Path, PathBuf};

use notify::EventKind;
use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

// === Audio file detection ===

#[test]
fn audio_file_mp3() {
    assert!(super::is_audio_file(Path::new("/music/song.mp3")));
}

#[test]
fn audio_file_flac() {
    assert!(super::is_audio_file(Path::new("/music/song.flac")));
}

#[test]
fn audio_file_m4a() {
    assert!(super::is_audio_file(Path::new("/music/song.m4a")));
}

#[test]
fn audio_file_ogg() {
    assert!(super::is_audio_file(Path::new("/music/song.ogg")));
}

#[test]
fn audio_file_wav() {
    assert!(super::is_audio_file(Path::new("/music/song.wav")));
}

#[test]
fn audio_file_aiff() {
    assert!(super::is_audio_file(Path::new("/music/song.aiff")));
}

#[test]
fn audio_file_case_insensitive() {
    assert!(super::is_audio_file(Path::new("/music/song.MP3")));
    assert!(super::is_audio_file(Path::new("/music/song.Flac")));
}

#[test]
fn non_audio_file_txt() {
    assert!(!super::is_audio_file(Path::new("/music/readme.txt")));
}

#[test]
fn non_audio_file_jpg() {
    assert!(!super::is_audio_file(Path::new("/music/cover.jpg")));
}

#[test]
fn no_extension() {
    assert!(!super::is_audio_file(Path::new("/music/noext")));
}

// === Event classification ===

#[test]
fn classify_create_file_audio() {
    let kind = EventKind::Create(CreateKind::File);
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], super::FileEvent::Created(p) if p == Path::new("/music/song.mp3"))
    );
}

#[test]
fn classify_create_file_non_audio_skipped() {
    let kind = EventKind::Create(CreateKind::File);
    let paths = vec![PathBuf::from("/music/readme.txt")];
    let events = super::classify_event(kind, &paths);
    assert!(events.is_empty());
}

#[test]
fn classify_create_any() {
    let kind = EventKind::Create(CreateKind::Any);
    let paths = vec![PathBuf::from("/music/song.flac")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], super::FileEvent::Created(_)));
}

#[test]
fn classify_remove_file() {
    let kind = EventKind::Remove(RemoveKind::File);
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], super::FileEvent::Removed(p) if p == Path::new("/music/song.mp3"))
    );
}

#[test]
fn classify_remove_any() {
    let kind = EventKind::Remove(RemoveKind::Any);
    let paths = vec![PathBuf::from("/music/song.wav")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], super::FileEvent::Removed(_)));
}

#[test]
fn classify_rename_both() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
    let paths = vec![PathBuf::from("/music/old.mp3"), PathBuf::from("/music/new.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        super::FileEvent::Renamed { from, to }
        if from == Path::new("/music/old.mp3") && to == Path::new("/music/new.mp3")
    ));
}

#[test]
fn classify_rename_from_as_removed() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::From));
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], super::FileEvent::Removed(_)));
}

#[test]
fn classify_rename_to_as_created() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::To));
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], super::FileEvent::Created(_)));
}

#[test]
fn classify_modify_data() {
    let kind = EventKind::Modify(ModifyKind::Data(DataChange::Content));
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], super::FileEvent::Modified(_)));
}

#[test]
fn classify_modify_any() {
    let kind = EventKind::Modify(ModifyKind::Any);
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], super::FileEvent::Modified(_)));
}

#[test]
fn classify_access_skipped() {
    let kind = EventKind::Access(notify::event::AccessKind::Read);
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert!(events.is_empty());
}

#[test]
fn classify_metadata_change_skipped() {
    let kind = EventKind::Modify(ModifyKind::Metadata(notify::event::MetadataKind::Any));
    let paths = vec![PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert!(events.is_empty());
}

#[test]
fn classify_remove_non_audio_skipped() {
    let kind = EventKind::Remove(RemoveKind::File);
    let paths = vec![PathBuf::from("/music/cover.jpg")];
    let events = super::classify_event(kind, &paths);
    assert!(events.is_empty());
}

#[test]
fn classify_rename_audio_to_non_audio_as_removed() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
    let paths = vec![PathBuf::from("/music/song.mp3"), PathBuf::from("/music/song.bak")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], super::FileEvent::Removed(p) if p == Path::new("/music/song.mp3"))
    );
}

#[test]
fn classify_rename_non_audio_to_audio_as_created() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
    let paths = vec![PathBuf::from("/music/song.bak"), PathBuf::from("/music/song.mp3")];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], super::FileEvent::Created(p) if p == Path::new("/music/song.mp3"))
    );
}

#[test]
fn classify_rename_non_audio_to_non_audio_skipped() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
    let paths = vec![PathBuf::from("/music/readme.txt"), PathBuf::from("/music/readme.bak")];
    let events = super::classify_event(kind, &paths);
    assert!(events.is_empty());
}

// === Rescan flag contract ===
//
// The watcher's debouncer callback short-circuits on `event.need_rescan()`
// (kernel queue overflow on inotify, equivalent on other backends) and emits
// `FileEvent::RescanNeeded` instead of running `classify_event`. That
// contract rests on `notify::event::Event::new(kind).set_flag(Flag::Rescan)`
// yielding `need_rescan() == true` — this test pins that assumption so a
// silent breaking change in a future `notify` version trips CI rather than
// silently dropping kernel-overflow rescans in production.
#[test]
fn rescan_flag_round_trips_through_notify_event() {
    use notify::event::{Event, Flag};

    let event = Event::new(EventKind::Any).set_flag(Flag::Rescan);
    assert!(event.need_rescan());

    let normal = Event::new(EventKind::Create(CreateKind::File));
    assert!(!normal.need_rescan());
}

/// Every extension the library walk collects must also reach the watcher, or a format
/// scans on startup and then goes stale for the rest of the session.
#[test]
fn audio_file_covers_every_scanned_extension() {
    for ext in melodia_core::utils::audio_ext::AUDIO_EXTENSIONS {
        let path = PathBuf::from("music").join(format!("song.{ext}"));
        assert!(super::is_audio_file(&path), "the watcher ignores .{ext}");
    }
}

/// ALAC ships inside `.m4a`; nothing writes a bare `.alac`. Listing it cost the walk a
/// lookup per file and could only ever have produced a row lofty refuses to read.
#[test]
fn alac_is_not_an_audio_extension() {
    assert!(!super::is_audio_file(&PathBuf::from("music").join("song.alac")));
}

// === Access events, dropped ahead of the debouncer ===

/// What `SkipAccess` hands on of `events`, fed through it in order.
fn passed_on(events: Vec<notify::Result<notify::Event>>) -> Vec<notify::Result<notify::Event>> {
    use notify::EventHandler;

    let (tx, rx) = std::sync::mpsc::channel();
    let mut filter = super::SkipAccess(tx);
    for event in events {
        filter.handle_event(event);
    }
    drop(filter);
    rx.into_iter().collect()
}

/// Every folder a walk reads and every file a scan or playback opens raises one, and the
/// debouncer would queue each for nothing.
#[test]
fn an_access_event_never_reaches_the_debouncer() {
    use notify::event::{AccessKind, AccessMode};

    let accesses =
        [AccessKind::Open(AccessMode::Any), AccessKind::Read, AccessKind::Close(AccessMode::Write)];
    let events =
        accesses.into_iter().map(|kind| Ok(notify::Event::new(EventKind::Access(kind)))).collect();

    let passed = passed_on(events);

    assert!(passed.is_empty(), "handed on {passed:?}");
}

#[test]
fn a_change_event_reaches_the_debouncer() {
    let kinds = [
        EventKind::Create(CreateKind::File),
        EventKind::Modify(ModifyKind::Data(DataChange::Content)),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
        EventKind::Remove(RemoveKind::File),
    ];
    let events = kinds.iter().map(|&kind| Ok(notify::Event::new(kind))).collect();

    let passed: Vec<Option<EventKind>> =
        passed_on(events).into_iter().map(|event| event.ok().map(|event| event.kind)).collect();

    assert_eq!(passed, kinds.map(Some));
}

/// The rescan flag rides an event of any kind, an access included, and is the one thing about
/// such an event that matters.
#[test]
fn an_access_event_asking_for_a_rescan_reaches_the_debouncer() {
    use notify::event::{AccessKind, Flag};

    let rescan = notify::Event::new(EventKind::Access(AccessKind::Read)).set_flag(Flag::Rescan);

    let passed: Vec<Option<bool>> = passed_on(vec![Ok(rescan)])
        .into_iter()
        .map(|event| event.ok().map(|event| event.need_rescan()))
        .collect();

    assert_eq!(passed, [Some(true)]);
}

/// A kernel queue overflow and a failing watch both mean the library may have moved unseen, and
/// dropping either leaves it stale until the next launch.
#[test]
fn a_rescan_flag_and_a_watcher_error_reach_the_debouncer() {
    use notify::event::Flag;

    let rescan = notify::Event::new(EventKind::Other).set_flag(Flag::Rescan);
    let events = vec![Ok(rescan), Err(notify::Error::generic("watch lost"))];

    let passed: Vec<Option<bool>> = passed_on(events)
        .into_iter()
        .map(|event| event.ok().map(|event| event.need_rescan()))
        .collect();

    assert_eq!(passed, [Some(true), None], "(the rescan, the error)");
}

// === Starting the watcher ===
//
// Everything above classifies an event `notify` already delivered. These two are the wiring that
// decides whether one is delivered at all, and it is the half no amount of `classify_event`
// coverage reaches: a `start` that refuses, or that watches nothing, leaves the library silently
// out of step with the disk until the next manual rescan.

use std::time::Duration;

use melodia_core::error::AppError;
use tempfile::TempDir;
use tokio::sync::mpsc;

use super::super::watcher::{FileEvent, FolderWatcher};

/// Five times the debouncer's own window, which is headroom for a loaded runner and not a
/// measurement: the `await` is what waits, so a passing run never reaches this and a broken one
/// spends all of it. Both halves are why it is not larger.
const BEFORE_GIVING_UP: Duration = Duration::from_secs(10);

/// A library folder the user unplugged, renamed, or has not created yet. Every one of those is
/// ordinary, and refusing the whole call over it leaves every *other* folder unwatched too.
#[tokio::test]
async fn a_folder_that_is_not_there_is_skipped_rather_than_refused() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (tx, _rx) = mpsc::channel(8);
    let mut watcher = FolderWatcher::new(tx);

    watcher.start(&[tmp.path().join("unplugged")])?;
    Ok(())
}

/// The whole point of the module, end to end and against the real `notify` backend: a file
/// appearing under a watched folder reaches the processor as a `Created`. Nothing below this
/// level can say the watch was actually registered, only that an event would classify correctly
/// if one arrived.
///
/// It spends the debouncer's window once, which is why there is one of these and not four. The
/// wait is an `await` on the channel under a ceiling rather than a sleep of its own, so the
/// duration above bounds a failure instead of timing the pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_appearing_under_a_watched_folder_reaches_the_channel() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (tx, mut rx) = mpsc::channel(8);
    let mut watcher = FolderWatcher::new(tx);
    watcher.start(&[tmp.path().to_path_buf()])?;

    let arriving = tmp.path().join("arriving.mp3");
    std::fs::write(&arriving, b"not really audio")?;

    let event = tokio::time::timeout(BEFORE_GIVING_UP, rx.recv()).await;

    assert!(
        matches!(&event, Ok(Some(FileEvent::Created(path))) if *path == arriving),
        "expected a Created for {}, got {event:?}",
        arriving.display()
    );
    Ok(())
}
