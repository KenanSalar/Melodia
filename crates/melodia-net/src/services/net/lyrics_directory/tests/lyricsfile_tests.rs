//! A stamp written wrong here lights every word of a record at the wrong time; a document refused
//! costs only its word times.

use super::*;

/// A line timed word by word, past a minute so each field of a stamp is written apart.
const SUNG: &str = "  - text: Hello world
    start_ms: 72505
    end_ms: 74000
    words:
      - text: \"Hello \"
        start_ms: 72505
      - text: world
        start_ms: 73250
";

fn document(lines: &[&str]) -> String {
    format!("lines:\n{}", lines.concat())
}

#[test]
fn a_word_timed_line_becomes_its_line_stamp_a_stamp_per_word_and_its_end() {
    assert_eq!(
        enhanced_lrc(&document(&[SUNG])).as_deref(),
        Some("[01:12.505]<01:12.505>Hello <01:13.250>world<01:14.000>\n")
    );
}

#[test]
fn a_line_with_no_text_becomes_a_blank_stamp() {
    // A blank stamp ends the line above, where a break's notes start.
    let pause = "  - text: \"\"\n    start_ms: 80000\n";

    assert_eq!(
        enhanced_lrc(&document(&[SUNG, pause])).as_deref(),
        Some("[01:12.505]<01:12.505>Hello <01:13.250>world<01:14.000>\n[01:20.000]\n")
    );
}

#[test]
fn a_line_without_words_keeps_its_text_under_its_own_stamp() {
    let spoken = "  - text: \"  spoken  \"\n    start_ms: 80000\n";

    assert_eq!(
        enhanced_lrc(&document(&[SUNG, spoken])).as_deref(),
        Some("[01:12.505]<01:12.505>Hello <01:13.250>world<01:14.000>\n[01:20.000]spoken\n")
    );
}

#[test]
fn a_word_timed_line_with_no_end_writes_no_end_stamp() {
    let open_ended = "  - text: one
    start_ms: 1000
    words:
      - text: one
        start_ms: 1000
";

    assert_eq!(
        enhanced_lrc(&document(&[open_ended])).as_deref(),
        Some("[00:01.000]<00:01.000>one\n")
    );
}

#[test]
fn a_document_timing_no_word_leaves_the_record_s_synced_lyrics_to_answer() {
    let spoken = "  - text: spoken\n    start_ms: 1000\n";

    assert_eq!(enhanced_lrc(&document(&[spoken])), None);
}

#[test]
fn a_document_that_does_not_read_leaves_the_record_s_synced_lyrics_to_answer() {
    let malformed = "  - text: one\n    start_ms: soon\n";

    assert_eq!(enhanced_lrc(&document(&[malformed])), None);
}
