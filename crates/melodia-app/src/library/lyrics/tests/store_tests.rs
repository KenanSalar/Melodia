//! What the store remembers, what it forgets, and which of the three answers each name carries.

use super::*;
use std::time::SystemTime;

use melodia_core::entities::cue::CueTrack;

const TRACK: &str = "/music/Artist/Album/01 Song.mp3";

fn track() -> LyricsKey {
    LyricsKey::whole_file(TRACK.to_owned())
}

/// Backdates a file so an expiry can be reached without waiting for one.
fn backdate(path: &Path, by: Duration) -> Result<(), AppError> {
    let when = SystemTime::now()
        .checked_sub(by)
        .ok_or_else(|| AppError::validation_msg("clock predates the window"))?;
    std::fs::File::options().write(true).open(path)?.set_modified(when)?;
    Ok(())
}

#[test]
fn an_empty_store_answers_nothing() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    assert!(read(tmp.path(), &track()).is_none());
    Ok(())
}

#[test]
fn a_sheet_round_trips_with_its_timings() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Sheet("[00:01.00]first\n[00:02.00]second\n"))?;

    let found = read(tmp.path(), &track());
    let Some(LyricsOutcome::Sheet(sheet)) = found else {
        return Err(AppError::validation_msg("expected a sheet"));
    };
    assert_eq!(sheet.lines.len(), 2);
    assert!(sheet.is_synced(), "a timed sheet must come back timed, or the panel stops following");
    Ok(())
}

#[test]
fn a_stored_sheet_is_marked_as_having_come_from_the_directory() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Sheet("[00:01.00]a\n"))?;

    let found = read(tmp.path(), &track());
    let Some(LyricsOutcome::Sheet(sheet)) = found else {
        return Err(AppError::validation_msg("expected a sheet"));
    };
    assert_eq!(sheet.source, LyricsSource::Online);
    Ok(())
}

#[test]
fn an_instrumental_is_its_own_answer() -> Result<(), AppError> {
    // Not a miss: it deserves different copy and it is never retried.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Instrumental)?;

    assert_eq!(read(tmp.path(), &track()), Some(LyricsOutcome::Instrumental));
    Ok(())
}

#[test]
fn a_miss_is_remembered_so_the_same_track_costs_one_request() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Nothing)?;

    assert_eq!(read(tmp.path(), &track()), Some(LyricsOutcome::Absent));
    Ok(())
}

#[test]
fn a_sheet_outranks_a_marker_left_beside_it() -> Result<(), AppError> {
    // A track asked about before it had a sheet keeps both names; the sheet is the later answer.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Nothing)?;
    write(tmp.path(), &track(), Fetched::Sheet("[00:01.00]a\n"))?;

    assert!(matches!(read(tmp.path(), &track()), Some(LyricsOutcome::Sheet(_))));
    Ok(())
}

#[test]
fn a_miss_older_than_its_window_is_asked_again() -> Result<(), AppError> {
    // The directory is a volunteer corpus that grows, so a miss is a statement about today.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Nothing)?;
    backdate(
        &entry(tmp.path(), &stem(&track()), ABSENT_EXT),
        MISS_STANDS_FOR + Duration::from_hours(1),
    )?;

    assert!(read(tmp.path(), &track()).is_none());
    Ok(())
}

#[test]
fn a_miss_inside_its_window_still_stands() -> Result<(), AppError> {
    // The step on the other side of the same boundary.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Nothing)?;
    let just_inside = MISS_STANDS_FOR.saturating_sub(Duration::from_hours(1));
    backdate(&entry(tmp.path(), &stem(&track()), ABSENT_EXT), just_inside)?;

    assert_eq!(read(tmp.path(), &track()), Some(LyricsOutcome::Absent));
    Ok(())
}

#[test]
fn an_instrumental_does_not_expire() -> Result<(), AppError> {
    // A permanent fact about the recording, unlike a miss.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Instrumental)?;
    backdate(
        &entry(tmp.path(), &stem(&track()), INSTRUMENTAL_EXT),
        MISS_STANDS_FOR + Duration::from_hours(1),
    )?;

    assert_eq!(read(tmp.path(), &track()), Some(LyricsOutcome::Instrumental));
    Ok(())
}

#[test]
fn the_key_is_the_track_path_and_nothing_else() {
    // Deliberately not the content hash the artwork store takes: a tag edit rewrites the file and
    // moves that, which would orphan a sheet every time a title typo was corrected.
    assert_eq!(stem(&track()), stem(&track()));
    let next = LyricsKey::whole_file("/music/Artist/Album/02 Song.mp3".to_owned());
    assert_ne!(stem(&track()), stem(&next));
}

#[test]
fn the_key_is_a_readable_hex_name() {
    let stem = stem(&track());
    assert_eq!(stem.len(), HASH_HEX_LEN);
    assert!(stem.bytes().all(|b| b.is_ascii_hexdigit()), "a directory listing has to be readable");
}

#[test]
fn the_prune_retires_a_stale_miss() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Nothing)?;
    let marker = entry(tmp.path(), &stem(&track()), ABSENT_EXT);
    backdate(&marker, MISS_STANDS_FOR + Duration::from_hours(1))?;

    assert_eq!(prune(tmp.path())?, 1);
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn the_prune_keeps_a_sheet_and_a_fresh_miss() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Sheet("[00:01.00]a\n"))?;
    let other = LyricsKey::whole_file("/music/other.flac".to_owned());
    write(tmp.path(), &other, Fetched::Nothing)?;

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

#[test]
fn a_refresh_drops_every_name_the_track_had() -> Result<(), AppError> {
    // The whole of what a refresh is. Leaving any one of the three behind lets the answer the user
    // asked to be rid of go on standing in for the lookup.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Sheet("[00:01.00]a\n"))?;
    write(tmp.path(), &track(), Fetched::Instrumental)?;
    write(tmp.path(), &track(), Fetched::Nothing)?;

    forget(tmp.path(), &track())?;

    assert!(read(tmp.path(), &track()).is_none());
    for extension in [SHEET_EXT, INSTRUMENTAL_EXT, ABSENT_EXT] {
        assert!(!entry(tmp.path(), &stem(&track()), extension).exists(), "{extension} survived");
    }
    Ok(())
}

#[test]
fn a_refresh_for_a_track_the_store_never_answered_is_not_a_failure() -> Result<(), AppError> {
    // The menu row is offered whatever the panel is showing, so it is routinely pressed on a
    // sheet that came off the file itself.
    let tmp = tempfile::TempDir::new()?;
    forget(tmp.path(), &track())?;
    Ok(())
}

#[test]
fn a_refresh_leaves_every_other_track_alone() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    let neighbour = LyricsKey::whole_file("/music/Artist/Album/02 Other.mp3".to_owned());
    write(tmp.path(), &track(), Fetched::Sheet("[00:01.00]a\n"))?;
    write(tmp.path(), &neighbour, Fetched::Sheet("[00:01.00]b\n"))?;

    forget(tmp.path(), &track())?;

    assert!(read(tmp.path(), &neighbour).is_some());
    Ok(())
}

#[test]
fn the_text_side_answers_with_the_sheet_as_it_was_stored() -> Result<(), AppError> {
    // Raw rather than re-serialized, so an offset tag and a gloss separator survive a promotion
    // into the file's own tag.
    let tmp = tempfile::TempDir::new()?;
    let sheet = "[offset:+500]\n[00:01.00]a^gloss\n";
    write(tmp.path(), &track(), Fetched::Sheet(sheet))?;

    assert_eq!(read_text(tmp.path(), &track()).as_deref(), Some(sheet));
    Ok(())
}

#[test]
fn the_text_side_is_never_answered_by_a_marker() -> Result<(), AppError> {
    // Both markers are written as empty files, so a text reader that only checked for existence
    // would offer the editor an empty sheet to overwrite the file's own tag with.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Instrumental)?;
    assert_eq!(read_text(tmp.path(), &track()), None, "an instrumental");

    write(tmp.path(), &track(), Fetched::Nothing)?;
    assert_eq!(read_text(tmp.path(), &track()), None, "a miss");
    Ok(())
}

#[test]
fn the_text_side_answers_nothing_for_a_track_the_store_has_not_seen() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    assert_eq!(read_text(tmp.path(), &track()), None);
    Ok(())
}

#[test]
fn a_sheet_that_parses_to_nothing_falls_through_to_the_marker_beside_it() -> Result<(), AppError> {
    // A stored file is read and parsed rather than merely found, so a sheet that has become
    // unreadable is a miss the next lookup can correct rather than a permanent empty panel.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Instrumental)?;
    std::fs::write(entry(tmp.path(), &stem(&track()), SHEET_EXT), "[ti:Song]\n")?;

    assert_eq!(read(tmp.path(), &track()), Some(LyricsOutcome::Instrumental));
    Ok(())
}

#[test]
fn a_miss_stamped_in_the_future_is_treated_as_spent() -> Result<(), AppError> {
    // A clock that moved backwards, or a file copied off a machine whose clock was ahead. The
    // window is measured with `elapsed`, which refuses a future instant, and the safe reading of
    // a refusal is to ask the directory again rather than to suppress the track forever.
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Nothing)?;
    let marker = entry(tmp.path(), &stem(&track()), ABSENT_EXT);
    let ahead = SystemTime::now()
        .checked_add(Duration::from_hours(24))
        .ok_or_else(|| AppError::validation_msg("clock cannot reach forward"))?;
    std::fs::File::options().write(true).open(&marker)?.set_modified(ahead)?;

    assert!(read(tmp.path(), &track()).is_none());
    Ok(())
}

/// Track `number` of a cue sheet over [`TRACK`], every track of which shares its path.
fn sheet_track(number: i64) -> LyricsKey {
    LyricsKey { path: TRACK.to_owned(), cue_track: CueTrack::try_from(number).ok() }
}

#[test]
fn each_track_of_one_image_keeps_its_own_answer() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &sheet_track(1), Fetched::Sheet("[00:01.00]first\n"))?;
    write(tmp.path(), &sheet_track(2), Fetched::Instrumental)?;

    let answers = [
        read(tmp.path(), &sheet_track(1)),
        read(tmp.path(), &sheet_track(2)),
        read(tmp.path(), &track()),
    ];

    assert!(
        matches!(answers, [Some(LyricsOutcome::Sheet(_)), Some(LyricsOutcome::Instrumental), None]),
        "{answers:?}"
    );
    Ok(())
}

#[test]
fn forgetting_one_track_of_an_image_leaves_its_neighbours() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &sheet_track(1), Fetched::Instrumental)?;
    write(tmp.path(), &sheet_track(2), Fetched::Instrumental)?;

    forget(tmp.path(), &sheet_track(1))?;

    let answers = [read(tmp.path(), &sheet_track(1)), read(tmp.path(), &sheet_track(2))];
    assert!(matches!(answers, [None, Some(LyricsOutcome::Instrumental)]), "{answers:?}");
    Ok(())
}

#[test]
fn a_whole_file_s_answer_is_filed_under_its_path_alone() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    write(tmp.path(), &track(), Fetched::Instrumental)?;

    let named_by_path = blake3::hash(TRACK.as_bytes()).to_hex()[..HASH_HEX_LEN].to_owned();

    assert!(entry(tmp.path(), &named_by_path, INSTRUMENTAL_EXT).exists());
    Ok(())
}
