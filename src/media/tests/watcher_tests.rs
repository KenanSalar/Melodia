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
    let paths = vec![
        PathBuf::from("/music/old.mp3"),
        PathBuf::from("/music/new.mp3"),
    ];
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
    let paths = vec![
        PathBuf::from("/music/song.mp3"),
        PathBuf::from("/music/song.bak"),
    ];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], super::FileEvent::Removed(p) if p == Path::new("/music/song.mp3"))
    );
}

#[test]
fn classify_rename_non_audio_to_audio_as_created() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
    let paths = vec![
        PathBuf::from("/music/song.bak"),
        PathBuf::from("/music/song.mp3"),
    ];
    let events = super::classify_event(kind, &paths);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], super::FileEvent::Created(p) if p == Path::new("/music/song.mp3"))
    );
}

#[test]
fn classify_rename_non_audio_to_non_audio_skipped() {
    let kind = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
    let paths = vec![
        PathBuf::from("/music/readme.txt"),
        PathBuf::from("/music/readme.bak"),
    ];
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
    for ext in crate::media::AUDIO_EXTENSIONS {
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
