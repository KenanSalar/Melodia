//! Which encoding a sheet is read in, how its lines split, and how an edited sheet is spelled
//! back: the untouched lines as the file held them, a new one in the sheet's own encoding where it
//! fits.

use std::path::Path;

use encoding_rs::{SHIFT_JIS, WINDOWS_1251};
use tempfile::TempDir;

use super::*;

fn sheet_text(album: &str, titles: &[&str]) -> String {
    let mut lines = vec![format!("TITLE \"{album}\""), "FILE \"album.wav\" WAVE".to_owned()];
    for (index, title) in titles.iter().enumerate() {
        lines.push(format!("  TRACK {:02} AUDIO", index + 1));
        lines.push(format!("    TITLE \"{title}\""));
        lines.push(format!("    INDEX 01 {:02}:00:00", index * 4));
    }
    lines.join("\r\n")
}

const RUSSIAN_TITLES: [&str; 4] =
    ["Группа крови", "Звезда по имени Солнце", "Пачка сигарет", "Спокойная ночь"];

const JAPANESE_TITLES: [&str; 4] = ["夜に駆ける", "群青", "ハルジオン", "たぶん"];

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn utf16be(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn with_prefix(prefix: &[u8], body: &[u8]) -> Vec<u8> {
    [prefix, body].concat()
}

#[test]
fn a_bom_names_the_encoding_and_its_length() {
    let text = "TITLE \"Album\"\r\n";
    let rows = [
        ("UTF-8", with_prefix(b"\xEF\xBB\xBF", text.as_bytes()), UTF_8, 3),
        ("UTF-16LE", with_prefix(b"\xFF\xFE", &utf16le(text)), UTF_16LE, 2),
        ("UTF-16BE", with_prefix(b"\xFE\xFF", &utf16be(text)), UTF_16BE, 2),
    ];
    for (label, bytes, encoding, bom) in rows {
        assert_eq!(detect(&bytes), (encoding, bom), "{label}");
    }
}

#[test]
fn utf16_with_no_bom_is_told_by_its_nuls() {
    let text = sheet_text("Album", &["One", "Two"]);
    let rows = [("LE", utf16le(&text), UTF_16LE), ("BE", utf16be(&text), UTF_16BE)];
    for (label, bytes, encoding) in rows {
        assert_eq!(detect(&bytes), (encoding, 0), "{label}");
    }
}

#[test]
fn utf8_with_no_bom_is_taken_as_it_stands() {
    let text = sheet_text("Café", &JAPANESE_TITLES);
    assert_eq!(detect(text.as_bytes()), (UTF_8, 0));
}

#[test]
fn a_codepage_sheet_is_guessed_from_its_text() {
    let rows = [
        ("Windows-1251", sheet_text("Кино", &RUSSIAN_TITLES), WINDOWS_1251),
        ("Shift_JIS", sheet_text("ヨルシカ", &JAPANESE_TITLES), SHIFT_JIS),
    ];
    for (label, text, encoding) in rows {
        let (bytes, _, unmappable) = encoding.encode(&text);
        assert!(!unmappable, "{label}: the fixture has to fit its own encoding");
        assert_eq!(detect(&bytes), (encoding, 0), "{label}");
    }
}

#[test]
fn a_codepage_sheet_reads_as_its_text() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("album.cue");
    let text = sheet_text("Кино", &RUSSIAN_TITLES);
    std::fs::write(&path, WINDOWS_1251.encode(&text).0)?;

    assert_eq!(read_sheet(&path)?, text);
    Ok(())
}

#[test]
fn a_bom_is_not_read_as_text() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("album.cue");
    std::fs::write(&path, with_prefix(b"\xEF\xBB\xBF", b"TITLE \"Album\""))?;

    assert_eq!(read_sheet(&path)?, "TITLE \"Album\"");
    Ok(())
}

#[test]
fn a_sheet_is_read_up_to_the_cap_and_refused_past_it() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let at_cap = tmp.path().join("at-cap.cue");
    let past_cap = tmp.path().join("past-cap.cue");
    std::fs::File::create(&at_cap)?.set_len(MAX_SHEET_BYTES)?;
    std::fs::File::create(&past_cap)?.set_len(MAX_SHEET_BYTES + 1)?;

    assert!(EncodedSheet::read(&at_cap).is_ok(), "a sheet at the cap is read");
    assert!(
        matches!(EncodedSheet::read(&past_cap), Err(AppError::Metadata { .. })),
        "a byte past it is refused"
    );
    Ok(())
}

fn split(sheet: &EncodedSheet) -> Vec<(String, &'static str)> {
    sheet.lines().into_iter().map(|line| (line.text, line.ending.as_str())).collect()
}

#[test]
fn each_line_keeps_its_own_ending() {
    let sheet = EncodedSheet::from_text("CRLF\r\nLF\nlast");

    assert_eq!(
        split(&sheet),
        [("CRLF".to_owned(), "\r\n"), ("LF".to_owned(), "\n"), ("last".to_owned(), "")]
    );
}

/// `Ċ` is U+010A, whose low byte is an LF's: split byte by byte, UTF-16 would break inside it.
#[test]
fn utf16_splits_on_whole_code_units() {
    let sheet = held(UTF_16LE, with_prefix(b"\xFF\xFE", &utf16le("TITLE \"Ċ\"\nREM X")), 2);

    assert_eq!(split(&sheet), [("TITLE \"Ċ\"".to_owned(), "\n"), ("REM X".to_owned(), "")]);
}

#[test]
fn a_bom_held_in_text_is_kept_off_the_first_line() {
    let sheet = EncodedSheet::from_text("\u{FEFF}TITLE \"Album\"");
    assert_eq!(split(&sheet), [("TITLE \"Album\"".to_owned(), "")]);
}

/// A sheet already read as `encoding`, so a test of the split or the encode doesn't lean on the
/// guess.
fn held(encoding: &'static Encoding, bytes: Vec<u8>, bom: usize) -> EncodedSheet {
    EncodedSheet { bytes, encoding, bom }
}

fn shift_jis(text: &str) -> EncodedSheet {
    held(SHIFT_JIS, SHIFT_JIS.encode(text).0.into_owned(), 0)
}

fn with_line_2(sheet: &EncodedSheet, new: &str) -> Vec<u8> {
    let lines = sheet.lines();
    let out: Vec<OutLine<'_>> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| match index {
            1 => OutLine::New(new.to_owned(), line.ending),
            _ => OutLine::Kept(line),
        })
        .collect();
    sheet.encode(&out, Path::new("album.cue"))
}

#[test]
fn an_edit_that_fits_stays_in_the_sheet_s_encoding() {
    let original = sheet_text("ヨルシカ", &JAPANESE_TITLES);
    let sheet = shift_jis(&original);

    let written = with_line_2(&sheet, "FILE \"アルバム.wav\" WAVE");

    let expected = original.replacen("FILE \"album.wav\" WAVE", "FILE \"アルバム.wav\" WAVE", 1);
    assert_eq!(written, SHIFT_JIS.encode(&expected).0.into_owned());
}

/// `Shift_JIS` has no `é`.
#[test]
fn an_edit_that_does_not_fit_rewrites_the_sheet_as_utf8_with_a_bom() {
    let original = sheet_text("ヨルシカ", &JAPANESE_TITLES);
    let sheet = shift_jis(&original);

    let written = with_line_2(&sheet, "FILE \"café.wav\" WAVE");

    let expected = original.replacen("FILE \"album.wav\" WAVE", "FILE \"café.wav\" WAVE", 1);
    assert_eq!(written, with_prefix(b"\xEF\xBB\xBF", expected.as_bytes()));
}

#[test]
fn an_edit_to_a_utf16_sheet_is_spelled_in_utf16_behind_its_bom() {
    let bom = b"\xFF\xFE";
    let sheet = held(UTF_16LE, with_prefix(bom, &utf16le("REM A\r\nTITLE \"Old\"\r\nREM B")), 2);

    let written = with_line_2(&sheet, "TITLE \"New\"");

    assert_eq!(written, with_prefix(bom, &utf16le("REM A\r\nTITLE \"New\"\r\nREM B")));
}
