//! Edit Tags written into a `.cue`: which line each field lands on, that every line the edit
//! doesn't name comes back byte for byte, and what an edit the sheet already states writes. Each
//! expected sheet is spelled out whole, so a stray change anywhere in it fails.

use std::fs;

use encoding_rs::SHIFT_JIS;
use tempfile::TempDir;

use super::*;
use melodia_core::entities::credits::{ROLES, RoleCredit, RoleCredits};
use melodia_core::entities::genre::GenreList;

fn track(number: i64) -> CueTrack {
    CueTrack::try_from(number).unwrap_or(CueTrack::HIDDEN)
}

fn set<T>(value: T) -> FieldEdit<T> {
    FieldEdit::Set(value)
}

fn editing(tracks: &[i64]) -> SheetRows {
    SheetRows { edited: tracks.iter().copied().map(track).collect(), artists: Vec::new() }
}

fn rewrite_bytes(
    bytes: &[u8],
    edit: &SheetEdit,
    rows: &SheetRows,
) -> Result<Option<Vec<u8>>, AppError> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("album.cue");
    fs::write(&path, bytes)?;
    rewritten_sheet(&path, edit, rows)
}

fn rewrite(
    lines: &[&str],
    edit: &SheetEdit,
    rows: &SheetRows,
) -> Result<Option<Vec<String>>, AppError> {
    let written = rewrite_bytes(lines.join("\n").as_bytes(), edit, rows)?;
    Ok(written
        .map(|bytes| String::from_utf8_lossy(&bytes).split('\n').map(str::to_owned).collect()))
}

fn sheet_with(index: usize, line: &str) -> Vec<String> {
    let mut lines: Vec<String> = SHEET.iter().map(|&line| line.to_owned()).collect();
    lines[index] = line.to_owned();
    lines
}

fn sheet_without(index: usize) -> Vec<String> {
    let mut lines: Vec<String> = SHEET.iter().map(|&line| line.to_owned()).collect();
    lines.remove(index);
    lines
}

fn sheet_inserting(index: usize, line: &str) -> Vec<String> {
    let mut lines: Vec<String> = SHEET.iter().map(|&line| line.to_owned()).collect();
    lines.insert(index, line.to_owned());
    lines
}

const SHEET: [&str; 12] = [
    "REM GENRE \"Rock\"",
    "REM DATE 1999-05-04",
    "PERFORMER \"Disc Artist\"",
    "TITLE \"Album\"",
    "FILE \"album.wav\" WAVE",
    "  TRACK 01 AUDIO",
    "    TITLE \"One\"",
    "    INDEX 01 00:00:00",
    "  TRACK 02 AUDIO",
    "    TITLE \"Two\"",
    "    PERFORMER \"Guest\"",
    "    INDEX 01 04:00:00",
];

#[test]
fn a_track_s_title_replaces_its_own_line_alone() -> Result<(), AppError> {
    let edit = SheetEdit { title: set("Uno".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1]))?;

    assert_eq!(written, Some(sheet_with(6, "    TITLE \"Uno\"")));
    Ok(())
}

#[test]
fn the_album_replaces_the_sheet_s_title_line() -> Result<(), AppError> {
    let edit = SheetEdit { album: set("New Album".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1, 2]))?;

    assert_eq!(written, Some(sheet_with(3, "TITLE \"New Album\"")));
    Ok(())
}

#[test]
fn an_edit_the_sheet_already_states_writes_nothing() -> Result<(), AppError> {
    let edit = SheetEdit { title: set("One".to_owned()), ..SheetEdit::default() };

    assert_eq!(rewrite(&SHEET, &edit, &editing(&[1]))?, None);
    Ok(())
}

#[test]
fn a_cleared_field_drops_its_lines() -> Result<(), AppError> {
    let edit = SheetEdit { artist: FieldEdit::Clear, ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[2]))?;

    assert_eq!(written, Some(sheet_without(10)));
    Ok(())
}

#[test]
fn an_empty_value_clears_the_field() -> Result<(), AppError> {
    let edit = SheetEdit { title: set("  ".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1]))?;

    assert_eq!(written, Some(sheet_without(6)));
    Ok(())
}

#[test]
fn a_set_writes_the_scope_s_first_line_of_its_command_and_drops_the_rest() -> Result<(), AppError> {
    let lines = [
        "FILE \"a.wav\" WAVE",
        "TRACK 01 AUDIO",
        "TITLE \"A\"",
        "TITLE \"B\"",
        "INDEX 01 00:00:00",
    ];
    let edit = SheetEdit { title: set("C".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&lines, &edit, &editing(&[1]))?;

    let expected = ["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", "TITLE \"C\"", "INDEX 01 00:00:00"];
    assert_eq!(written, Some(expected.map(str::to_owned).to_vec()));
    Ok(())
}

#[test]
fn a_field_a_track_lacks_goes_under_its_track_line() -> Result<(), AppError> {
    let edit = SheetEdit { isrc: set("USRC17607839".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1]))?;

    assert_eq!(written, Some(sheet_inserting(6, "    ISRC USRC17607839")));
    Ok(())
}

#[test]
fn a_field_the_sheet_lacks_goes_ahead_of_the_first_file_line() -> Result<(), AppError> {
    let edit = SheetEdit { barcode: set("0724384960650".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1, 2]))?;

    assert_eq!(written, Some(sheet_inserting(4, "CATALOG 0724384960650")));
    Ok(())
}

#[test]
fn free_text_is_quoted_and_a_number_or_code_written_bare() -> Result<(), AppError> {
    let edit = SheetEdit {
        disc_number: set("2".to_owned()),
        disc_total: set("3".to_owned()),
        ..SheetEdit::default()
    };

    let written = rewrite(&SHEET, &edit, &editing(&[1, 2]))?;

    let mut expected = sheet_inserting(4, "REM DISCNUMBER 2");
    expected.insert(5, "REM TOTALDISCS 3".to_owned());
    assert_eq!(written, Some(expected));
    Ok(())
}

#[test]
fn a_shared_field_over_every_track_is_written_once_for_the_sheet() -> Result<(), AppError> {
    let lines = [&SHEET[..11], &["    REM GENRE \"Funk\"", SHEET[11]]].concat();
    let edit = SheetEdit { genre: set("Jazz".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&lines, &edit, &editing(&[1, 2]))?;

    assert_eq!(written, Some(sheet_with(0, "REM GENRE \"Jazz\"")), "track 2's own genre goes");
    Ok(())
}

#[test]
fn a_shared_field_over_some_tracks_is_written_on_each() -> Result<(), AppError> {
    let edit = SheetEdit { genre: set("Jazz".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1]))?;

    assert_eq!(written, Some(sheet_inserting(6, "    REM GENRE \"Jazz\"")));
    Ok(())
}

#[test]
fn the_hidden_track_neither_takes_lines_nor_stops_an_edit_covering_the_sheet()
-> Result<(), AppError> {
    let edit = SheetEdit { genre: set("Jazz".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[0, 1, 2]))?;

    assert_eq!(written, Some(sheet_with(0, "REM GENRE \"Jazz\"")));
    Ok(())
}

#[test]
fn an_edit_naming_only_the_hidden_track_writes_nothing() -> Result<(), AppError> {
    let edit = SheetEdit { title: set("Hidden".to_owned()), ..SheetEdit::default() };

    assert_eq!(rewrite(&SHEET, &edit, &editing(&[0]))?, None);
    Ok(())
}

#[test]
fn a_year_keeps_the_month_and_day_the_date_had() -> Result<(), AppError> {
    let edit = SheetEdit { year: set(2001), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1, 2]))?;

    assert_eq!(written, Some(sheet_with(1, "REM DATE 2001-05-04")));
    Ok(())
}

#[test]
fn a_cleared_composer_takes_both_lines_a_composer_reads_from() -> Result<(), AppError> {
    let lines = [
        "SONGWRITER \"Writer\"",
        "REM COMPOSER \"Composer\"",
        "FILE \"a.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
    ];
    let edit = SheetEdit { composer: FieldEdit::Clear, ..SheetEdit::default() };

    let written = rewrite(&lines, &edit, &editing(&[1]))?;

    let expected = ["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", "INDEX 01 00:00:00"];
    assert_eq!(written, Some(expected.map(str::to_owned).to_vec()));
    Ok(())
}

#[test]
fn a_line_break_in_a_value_stays_on_one_line() -> Result<(), AppError> {
    let edit = SheetEdit { title: set("Part One\r\nPart Two".to_owned()), ..SheetEdit::default() };

    let written = rewrite(&SHEET, &edit, &editing(&[1]))?;

    assert_eq!(written, Some(sheet_with(6, "    TITLE \"Part One  Part Two\"")));
    Ok(())
}

#[test]
fn an_album_artist_edit_pins_each_inheriting_track_to_the_artist_it_shows() -> Result<(), AppError>
{
    let edit = SheetEdit { album_artist: set("New Artist".to_owned()), ..SheetEdit::default() };
    let rows = SheetRows {
        edited: vec![track(1)],
        artists: vec![(track(1), "Disc Artist".to_owned()), (track(2), "Guest".to_owned())],
    };

    let written = rewrite(&SHEET, &edit, &rows)?;

    let mut expected = sheet_with(2, "PERFORMER \"New Artist\"");
    expected.insert(6, "    PERFORMER \"Disc Artist\"".to_owned());
    assert_eq!(written, Some(expected));
    Ok(())
}

#[test]
fn an_album_artist_edit_pins_the_image_s_artist_where_the_sheet_states_none() -> Result<(), AppError>
{
    let lines = ["FILE \"a.wav\" WAVE", "TRACK 01 AUDIO", "INDEX 01 00:00:00"];
    let edit = SheetEdit { album_artist: set("New Artist".to_owned()), ..SheetEdit::default() };
    let rows =
        SheetRows { edited: vec![track(1)], artists: vec![(track(1), "Image Artist".to_owned())] };

    let written = rewrite(&lines, &edit, &rows)?;

    let expected = [
        "PERFORMER \"New Artist\"",
        "FILE \"a.wav\" WAVE",
        "TRACK 01 AUDIO",
        "PERFORMER \"Image Artist\"",
        "INDEX 01 00:00:00",
    ];
    assert_eq!(written, Some(expected.map(str::to_owned).to_vec()));
    Ok(())
}

#[test]
fn a_track_whose_artist_the_same_edit_sets_is_not_pinned() -> Result<(), AppError> {
    let edit = SheetEdit {
        artist: set("Solo".to_owned()),
        album_artist: set("New Artist".to_owned()),
        ..SheetEdit::default()
    };
    let rows = SheetRows {
        edited: vec![track(1)],
        artists: vec![(track(1), "Disc Artist".to_owned()), (track(2), "Guest".to_owned())],
    };

    let written = rewrite(&SHEET, &edit, &rows)?;

    let mut expected = sheet_with(2, "PERFORMER \"New Artist\"");
    expected.insert(6, "    PERFORMER \"Solo\"".to_owned());
    assert_eq!(written, Some(expected));
    Ok(())
}

#[test]
fn an_album_artist_the_sheet_already_states_pins_nothing() -> Result<(), AppError> {
    let edit = SheetEdit { album_artist: set("Disc Artist".to_owned()), ..SheetEdit::default() };
    let rows = SheetRows {
        edited: vec![track(1)],
        artists: vec![(track(1), "Disc Artist".to_owned()), (track(2), "Guest".to_owned())],
    };

    assert_eq!(rewrite(&SHEET, &edit, &rows)?, None);
    Ok(())
}

#[test]
fn every_byte_the_edit_does_not_name_comes_back_as_it_was() -> Result<(), AppError> {
    let original = b"\xEF\xBB\xBFTITLE \"Album\"\r\nFILE \"a.wav\" WAVE\r\n  TRACK 01 AUDIO\r\n    TITLE \"One\"\r\n    INDEX 01 00:00:00";
    let edit = SheetEdit {
        title: set("Uno".to_owned()),
        isrc: set("USRC17607839".to_owned()),
        ..SheetEdit::default()
    };

    let written = rewrite_bytes(original, &edit, &editing(&[1]))?;

    let expected = b"\xEF\xBB\xBFTITLE \"Album\"\r\nFILE \"a.wav\" WAVE\r\n  TRACK 01 AUDIO\r\n    ISRC USRC17607839\r\n    TITLE \"Uno\"\r\n    INDEX 01 00:00:00";
    assert_eq!(written.as_deref(), Some(&expected[..]));
    Ok(())
}

#[test]
fn a_codepage_sheet_is_written_back_in_its_codepage() -> Result<(), AppError> {
    let titles = ["夜に駆ける", "群青", "ハルジオン", "たぶん"];
    let mut lines = vec!["TITLE \"ヨルシカ\"".to_owned(), "FILE \"album.wav\" WAVE".to_owned()];
    for (index, title) in titles.iter().enumerate() {
        lines.push(format!("TRACK {:02} AUDIO", index + 1));
        lines.push(format!("TITLE \"{title}\""));
        lines.push(format!("INDEX 01 {:02}:00:00", index * 4));
    }
    let original = lines.join("\r\n");
    let edit = SheetEdit { title: set("春泥棒".to_owned()), ..SheetEdit::default() };

    let written = rewrite_bytes(&SHIFT_JIS.encode(&original).0, &edit, &editing(&[2]))?;

    let expected = original.replacen("TITLE \"群青\"", "TITLE \"春泥棒\"", 1);
    assert_eq!(written, Some(SHIFT_JIS.encode(&expected).0.into_owned()));
    Ok(())
}

fn composer(name: &str) -> RoleCredit {
    RoleCredit { role: CreditRole::Composer, name: name.to_owned(), detail: String::new() }
}

#[test]
fn a_tag_edit_s_lists_and_credits_become_one_value_each() {
    let edit = TagEdit {
        artist: set(ArtistCredit::from_tags("A feat. B", &["A".to_owned(), "B".to_owned()])),
        genres: set(GenreList::new(vec!["Rock".to_owned(), "Jazz".to_owned()])),
        credits: set(RoleCreditEdit::whole(RoleCredits::new(vec![composer("X"), composer("Y")]))),
        disc_number: set(2),
        ..TagEdit::default()
    };

    let sheet = SheetEdit::from(&edit);

    let expected = SheetEdit {
        artist: set("A feat. B".to_owned()),
        composer: set("X; Y".to_owned()),
        genre: set("Rock; Jazz".to_owned()),
        disc_number: set("2".to_owned()),
        ..SheetEdit::default()
    };
    assert_eq!(sheet, expected);
}

#[test]
fn credits_leaving_the_composer_out_of_scope_keep_it() {
    let mut lyricist_only = [false; ROLES.len()];
    lyricist_only[1] = true;
    let edit = TagEdit {
        credits: set(RoleCreditEdit::new(RoleCredits::default(), lyricist_only)),
        ..TagEdit::default()
    };

    assert_eq!(SheetEdit::from(&edit).composer, FieldEdit::Keep);
}

#[test]
fn credits_answering_for_the_composer_with_none_clear_it() {
    let edit = TagEdit {
        credits: set(RoleCreditEdit::whole(RoleCredits::default())),
        ..TagEdit::default()
    };

    assert_eq!(SheetEdit::from(&edit).composer, FieldEdit::Clear);
}

#[test]
fn a_rating_s_write_back_is_no_sheet_edit() {
    assert!(SheetEdit::from(&TagEdit::default()).is_empty());
}
