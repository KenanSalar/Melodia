//! The sheet as rippers write it: what the reader takes from each line, which scope a line
//! belongs to, and that a line it can't read costs only that line.

use std::path::Path;

use super::*;

fn read(lines: &[&str]) -> Sheet {
    parse(&lines.join("\n"), Path::new("album.cue"), Files::Named)
}

fn cue_track(number: i64) -> Option<CueTrack> {
    CueTrack::try_from(number).ok()
}

fn frames(count: u32) -> Option<CueFrames> {
    CueFrames::try_from(i64::from(count)).ok()
}

fn start_of(sheet: &Sheet, number: i64) -> Option<CueFrames> {
    let track = sheet.track(cue_track(number)?)?;
    track.index(1).map(|index| index.at)
}

fn title_of(sheet: &Sheet, number: i64) -> Option<&str> {
    let track = sheet.track(cue_track(number)?)?;
    track.fields.title.as_deref()
}

#[test]
fn keywords_read_in_any_case() {
    let sheet = read(&[
        "file \"album.wav\" wave",
        "  Track 01 Audio",
        "    title \"One\"",
        "    INDEX 01 00:00:00",
    ]);

    assert_eq!((title_of(&sheet, 1), start_of(&sheet, 1)), (Some("One"), frames(0)));
}

#[test]
fn a_value_reads_quoted_or_bare() {
    let rows = [
        ("TITLE \"Quoted Title\"", "Quoted Title"),
        ("TITLE Bare Title", "Bare Title"),
        ("TITLE \"Say \"Hi\" Now\"", "Say \"Hi\" Now"),
        ("TITLE \"Kept\" ripped by someone", "Kept"),
        ("TITLE \"Unclosed", "Unclosed"),
    ];
    for (line, title) in rows {
        let sheet = read(&[line]);
        assert_eq!(sheet.fields.title.as_deref(), Some(title), "{line}");
    }
}

#[test]
fn a_time_counts_minutes_seconds_and_frames() {
    let rows = [
        ("00:00:00", 0),
        ("00:00:74", 74),
        ("00:01:00", 75),
        ("01:00:00", 4_500),
        ("120:00:00", 540_000),
    ];
    for (time, at) in rows {
        let sheet = read(&["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", &format!("INDEX 01 {time}")]);
        assert_eq!(start_of(&sheet, 1), frames(at), "{time}");
    }
}

#[test]
fn a_time_out_of_range_or_misshapen_drops_its_line() {
    for time in ["00:60:00", "00:00:75", "00:00", "00:00:00:00", "aa:00:00", "-1:00:00", ""] {
        let sheet = read(&["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", &format!("INDEX 01 {time}")]);
        assert_eq!(start_of(&sheet, 1), None, "{time:?}");
    }
}

#[test]
fn a_bad_line_costs_only_itself() {
    let sheet = read(&[
        "FILE \"a.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 01 03:99:00",
        "TRACK 03 AUDIO",
        "INDEX 01 07:00:00",
    ]);

    assert_eq!(
        [start_of(&sheet, 1), start_of(&sheet, 2), start_of(&sheet, 3)],
        [frames(0), None, frames(31_500)]
    );
}

#[test]
fn junk_after_an_index_is_ignored() {
    let sheet = read(&["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", "INDEX 01 00:02:00 junk"]);
    assert_eq!(start_of(&sheet, 1), frames(150));
}

#[test]
fn a_file_name_reads_quoted_or_before_its_type() {
    let rows = [
        ("FILE \"My Album.flac\" WAVE", "My Album.flac"),
        ("FILE My Album.flac WAVE", "My Album.flac"),
        ("FILE album.wav", "album.wav"),
    ];
    for (line, name) in rows {
        assert_eq!(read(&[line]).files, [name], "{line}");
    }
}

#[test]
fn a_file_line_repeating_the_one_before_names_the_same_file() {
    let sheet = read(&[
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "FILE \"album.wav\" WAVE",
        "TRACK 02 AUDIO",
        "INDEX 01 03:00:00",
    ]);

    assert_eq!(sheet.files, ["album.wav"]);
}

#[test]
fn a_file_named_again_after_another_is_a_third_file_line() {
    let sheet = read(&["FILE \"a.wav\" WAVE", "FILE \"b.wav\" WAVE", "FILE \"a.wav\" WAVE"]);
    assert_eq!(sheet.files, ["a.wav", "b.wav", "a.wav"]);
}

#[test]
fn an_index_sits_in_the_file_above_it() {
    let sheet = read(&[
        "FILE \"01.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 00 03:58:00",
        "FILE \"02.wav\" WAVE",
        "INDEX 01 00:00:00",
    ]);
    let track = cue_track(2).and_then(|number| sheet.track(number));

    let files = track.map(|t| (t.index(0).map(|i| i.file), t.index(1).map(|i| i.file)));

    assert_eq!(files, Some((Some(0), Some(1))));
}

#[test]
fn an_index_above_every_file_line_is_dropped() {
    let sheet = read(&["TRACK 01 AUDIO", "INDEX 01 00:00:00", "FILE \"a.wav\" WAVE"]);
    assert_eq!(start_of(&sheet, 1), None);
}

#[test]
fn a_sheet_inside_its_file_reads_every_index_in_that_file() {
    let text = ["FILE \"whatever.wav\" WAVE", "TRACK 01 AUDIO", "INDEX 01 00:00:00"].join("\n");

    let sheet = parse(&text, Path::new("album.flac"), Files::Inside);

    let file = sheet.tracks.first().and_then(|track| track.index(1)).map(|index| index.file);
    assert_eq!((sheet.files, file), (vec![String::new()], Some(0)));
}

#[test]
fn a_line_belongs_to_the_sheet_until_the_first_track_then_to_the_track_above_it() {
    let sheet = read(&[
        "REM GENRE \"Sheet Genre\"",
        "PERFORMER \"Disc Artist\"",
        "FILE \"a.wav\" WAVE",
        "TRACK 01 AUDIO",
        "REM GENRE \"Track Genre\"",
        "PERFORMER \"Track Artist\"",
        "INDEX 01 00:00:00",
    ]);
    let track = cue_track(1).and_then(|number| sheet.track(number));

    let sheet_scope = (sheet.fields.rem(rem::GENRE), sheet.fields.performer.as_deref());
    let track_scope = track.map(|t| (t.fields.rem(rem::GENRE), t.fields.performer.as_deref()));

    assert_eq!(sheet_scope, (Some("Sheet Genre"), Some("Disc Artist")));
    assert_eq!(track_scope, Some((Some("Track Genre"), Some("Track Artist"))));
}

#[test]
fn a_rem_key_reads_in_any_case() {
    let sheet = read(&["rem genre Ambient", "Rem Date 1999"]);
    assert_eq!(
        (sheet.fields.rem(rem::GENRE), sheet.fields.rem(rem::DATE)),
        (Some("Ambient"), Some("1999"))
    );
}

#[test]
fn a_blank_repeat_keeps_the_value_read_first() {
    let sheet = read(&["TITLE \"Album\"", "TITLE \"\"", "TITLE"]);
    assert_eq!(sheet.fields.title.as_deref(), Some("Album"));
}

#[test]
fn isrc_belongs_to_its_track_and_catalog_to_the_sheet() {
    let sheet = read(&[
        "CATALOG 0724384960650",
        "FILE \"a.wav\" WAVE",
        "TRACK 01 AUDIO",
        "ISRC USRC17607839",
        "INDEX 01 00:00:00",
    ]);
    let isrc = cue_track(1).and_then(|number| sheet.track(number)).and_then(|t| t.isrc.clone());

    assert_eq!(
        (sheet.catalog.as_deref(), isrc.as_deref()),
        (Some("0724384960650"), Some("USRC17607839"))
    );
}

#[test]
fn a_track_is_audio_unless_its_mode_says_otherwise() {
    let sheet = read(&["TRACK 01 AUDIO", "TRACK 02 MODE1/2352", "TRACK 03", "TRACK 04 audio"]);
    let audio: Vec<(u8, bool)> =
        sheet.tracks.iter().map(|track| (track.number.number(), track.is_audio)).collect();
    assert_eq!(audio, [(1, true), (2, false), (3, true), (4, true)]);
}

/// The hidden track is the layout's to find; a sheet numbering one 0, or past 99, names no track
/// a row could key on.
#[test]
fn a_track_numbered_outside_one_to_ninety_nine_is_refused() {
    let sheet = read(&["TRACK 00 AUDIO", "TRACK 100 AUDIO", "TRACK xx AUDIO", "TRACK 99 AUDIO"]);
    let numbers: Vec<u8> = sheet.tracks.iter().map(|track| track.number.number()).collect();
    assert_eq!(numbers, [99]);
}

#[test]
fn the_lines_under_a_refused_track_are_nobody_s() {
    let sheet = read(&[
        "FILE \"a.wav\" WAVE",
        "TRACK 01 AUDIO",
        "TITLE \"One\"",
        "INDEX 01 00:00:00",
        "TRACK 00 AUDIO",
        "TITLE \"Stray\"",
        "INDEX 01 04:00:00",
        "TRACK 02 AUDIO",
        "TITLE \"Two\"",
        "INDEX 01 05:00:00",
    ]);

    assert_eq!(
        [(title_of(&sheet, 1), start_of(&sheet, 1)), (title_of(&sheet, 2), start_of(&sheet, 2))],
        [(Some("One"), frames(0)), (Some("Two"), frames(22_500))]
    );
}

#[test]
fn a_sheet_places_a_track_only_with_an_index_01() {
    let rows: [(&[&str], bool); 3] = [
        (&["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", "INDEX 01 00:00:00"], true),
        (&["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", "INDEX 00 00:00:00"], false),
        (&["TITLE \"Just a title\""], false),
    ];
    for (lines, places) in rows {
        assert_eq!(read(lines).places_a_track(), places, "{lines:?}");
    }
}

#[test]
fn a_stated_value_reads_past_a_rem_key() {
    let rows = [
        ("    REM GENRE \"Rock\"", Some("Rock")),
        ("TITLE \"Song\"", Some("Song")),
        ("ISRC USRC17607839", Some("USRC17607839")),
        ("REM GENRE", None),
        ("TITLE \"\"", None),
    ];
    for (line, value) in rows {
        assert_eq!(stated_value(line).as_deref(), value, "{line}");
    }
}
