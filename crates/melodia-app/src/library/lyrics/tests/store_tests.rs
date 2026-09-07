//! What the store remembers, what it forgets, and which of the three answers each name carries.

use super::*;
use std::time::SystemTime;

const TRACK: &str = "/music/Artist/Album/01 Song.mp3";

/// Backdates a file so an expiry can be reached without waiting for one.
fn backdate(path: &Path, by: Duration) -> Result<(), AppError> {
    let when = SystemTime::now()
        .checked_sub(by)
        .ok_or_else(|| AppError::Validation("clock predates the window".into()))?;
    std::fs::File::options().write(true).open(path)?.set_modified(when)?;
    Ok(())
}

#[test]
fn an_empty_store_answers_nothing() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    assert!(read(tmp.path(), TRACK).is_none());
    Ok(())
}

#[test]
fn a_sheet_round_trips_with_its_timings() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Sheet("[00:01.00]first\n[00:02.00]second\n"))?;

    let found = read(tmp.path(), TRACK);
    let Some(LyricsOutcome::Sheet(sheet)) = found else {
        return Err(AppError::Validation("expected a sheet".into()));
    };
    assert_eq!(sheet.lines.len(), 2);
    assert!(sheet.is_synced(), "a timed sheet must come back timed, or the panel stops following");
    Ok(())
}

#[test]
fn a_stored_sheet_is_marked_as_having_come_from_the_directory() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Sheet("[00:01.00]a\n"))?;

    let found = read(tmp.path(), TRACK);
    let Some(LyricsOutcome::Sheet(sheet)) = found else {
        return Err(AppError::Validation("expected a sheet".into()));
    };
    assert_eq!(sheet.source, LyricsSource::Online);
    Ok(())
}

#[test]
fn an_instrumental_is_its_own_answer() -> Result<(), AppError> {
    // Not a miss: it deserves different copy and it is never retried.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Instrumental)?;

    assert_eq!(read(tmp.path(), TRACK), Some(LyricsOutcome::Instrumental));
    Ok(())
}

#[test]
fn a_miss_is_remembered_so_the_same_track_costs_one_request() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Nothing)?;

    assert_eq!(read(tmp.path(), TRACK), Some(LyricsOutcome::Absent));
    Ok(())
}

#[test]
fn a_sheet_outranks_a_marker_left_beside_it() -> Result<(), AppError> {
    // A track asked about before it had a sheet keeps both names; the sheet is the later answer.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Nothing)?;
    write(tmp.path(), TRACK, Fetched::Sheet("[00:01.00]a\n"))?;

    assert!(matches!(read(tmp.path(), TRACK), Some(LyricsOutcome::Sheet(_))));
    Ok(())
}

#[test]
fn a_miss_older_than_its_window_is_asked_again() -> Result<(), AppError> {
    // The directory is a volunteer corpus that grows, so a miss is a statement about today.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Nothing)?;
    backdate(
        &entry(tmp.path(), &key(TRACK), ABSENT_EXT),
        MISS_STANDS_FOR + Duration::from_hours(1),
    )?;

    assert!(read(tmp.path(), TRACK).is_none());
    Ok(())
}

#[test]
fn a_miss_inside_its_window_still_stands() -> Result<(), AppError> {
    // The step on the other side of the same boundary.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Nothing)?;
    let just_inside = MISS_STANDS_FOR.saturating_sub(Duration::from_hours(1));
    backdate(&entry(tmp.path(), &key(TRACK), ABSENT_EXT), just_inside)?;

    assert_eq!(read(tmp.path(), TRACK), Some(LyricsOutcome::Absent));
    Ok(())
}

#[test]
fn an_instrumental_does_not_expire() -> Result<(), AppError> {
    // A permanent fact about the recording, unlike a miss.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Instrumental)?;
    backdate(
        &entry(tmp.path(), &key(TRACK), INSTRUMENTAL_EXT),
        MISS_STANDS_FOR + Duration::from_hours(1),
    )?;

    assert_eq!(read(tmp.path(), TRACK), Some(LyricsOutcome::Instrumental));
    Ok(())
}

#[test]
fn the_key_is_the_track_path_and_nothing_else() {
    // Deliberately not the content hash the artwork store takes: a tag edit rewrites the file and
    // moves that, which would orphan a sheet every time a title typo was corrected.
    assert_eq!(key(TRACK), key(TRACK));
    assert_ne!(key(TRACK), key("/music/Artist/Album/02 Song.mp3"));
}

#[test]
fn the_key_is_a_readable_hex_name() {
    let stem = key(TRACK);
    assert_eq!(stem.len(), HASH_HEX_LEN);
    assert!(stem.bytes().all(|b| b.is_ascii_hexdigit()), "a directory listing has to be readable");
}

#[test]
fn the_prune_retires_a_stale_miss() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Nothing)?;
    let marker = entry(tmp.path(), &key(TRACK), ABSENT_EXT);
    backdate(&marker, MISS_STANDS_FOR + Duration::from_hours(1))?;

    assert_eq!(prune(tmp.path())?, 1);
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn the_prune_keeps_a_sheet_and_a_fresh_miss() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), TRACK, Fetched::Sheet("[00:01.00]a\n"))?;
    write(tmp.path(), "/music/other.flac", Fetched::Nothing)?;

    assert_eq!(prune(tmp.path())?, 0, "nothing here is over its bounds");
    Ok(())
}

#[test]
fn the_prune_leaves_alone_what_it_did_not_write() -> Result<(), AppError> {
    // This directory is under the user's data root; a pass deleting whatever it found would
    // delete whatever someone else put there.
    let tmp = tempfile::TempDir::new()?;
    let stranger = tmp.path().join("notes.txt");
    std::fs::write(&stranger, b"mine")?;
    let wrong_stem = tmp.path().join("hello.lrc");
    std::fs::write(&wrong_stem, b"[00:01.00]a")?;

    assert_eq!(prune(tmp.path())?, 0);
    assert!(stranger.exists(), "a file the store never named");
    assert!(wrong_stem.exists(), "the extension is ours but the stem is not a key");
    Ok(())
}
