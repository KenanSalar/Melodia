//! lrclib's Lyricsfile, the YAML document a record keeps its word timings in, read into the
//! enhanced LRC the sheet parser takes. Converting here keeps the format inside the client: the
//! caller still gets LRC, whichever field the timings came from.

use std::fmt::Write as _;

use serde::Deserialize;

#[derive(Deserialize)]
struct Lyricsfile {
    #[serde(default)]
    lines: Vec<Line>,
}

/// One sung line. An empty `text` is a pause, written as a blank stamp so the line above ends
/// there.
#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    text: String,
    start_ms: u64,
    #[serde(default)]
    end_ms: Option<u64>,
    #[serde(default)]
    words: Vec<Word>,
}

/// Its `text` carries its own trailing space, which is what separates it from the next.
#[derive(Deserialize)]
struct Word {
    text: String,
    start_ms: u64,
}

/// The document as enhanced LRC, `None` where it doesn't read or times no word: the record's
/// `syncedLyrics` answers then. Times that don't add up are left to the parser, which lights such a
/// line whole.
pub(super) fn enhanced_lrc(document: &str) -> Option<String> {
    let Ok(file) = serde_saphyr::from_str::<Lyricsfile>(document) else {
        log::debug!("lyrics: a lyricsfile didn't read, so its record answers with its LRC");
        return None;
    };
    if file.lines.iter().all(|line| line.words.is_empty()) {
        return None;
    }

    let mut sheet = String::new();
    for line in &file.lines {
        write_line(&mut sheet, line);
    }
    Some(sheet)
}

/// A line timed word by word ends on its `end_ms`, so the notes of a long break start there. One
/// with no words keeps its text under the line stamp alone.
fn write_line(sheet: &mut String, line: &Line) {
    push_stamp(sheet, '[', line.start_ms, ']');
    if line.words.is_empty() {
        sheet.push_str(line.text.trim());
    } else {
        for word in &line.words {
            push_stamp(sheet, '<', word.start_ms, '>');
            sheet.push_str(&word.text);
        }
        if let Some(end_ms) = line.end_ms {
            push_stamp(sheet, '<', end_ms, '>');
        }
    }
    sheet.push('\n');
}

/// `mm:ss.mmm`: the parser reads three fraction digits, so a word keeps its millisecond.
fn push_stamp(sheet: &mut String, open: char, ms: u64, close: char) {
    let (minutes, within) = (ms / 60_000, ms % 60_000);
    // Writing into a String cannot fail.
    let _ = write!(sheet, "{open}{minutes:02}:{:02}.{:03}{close}", within / 1000, within % 1000);
}
