//! The format's boundaries, which is where a lyric sheet goes wrong: a stamp read one place out
//! runs a whole song's highlight late, and every mis-read here is silent.

use super::*;
use melodia_core::entities::lyrics::LyricsSource;

/// The sheet's stamps and text, in the order the panel would draw them.
fn drawn(text: &str) -> Vec<(Option<i64>, String)> {
    parse(text, LyricsSource::Sidecar).map_or_else(Vec::new, |sheet| {
        sheet.lines.into_iter().map(|line| (line.at_ms, line.text)).collect()
    })
}

/// Just the stamps, for the cases where the text is beside the point.
fn stamps(text: &str) -> Vec<Option<i64>> {
    drawn(text).into_iter().map(|(at, _)| at).collect()
}

/// Each line with its end, the one trace a blank or end stamp leaves on the sheet.
fn folded(text: &str) -> Vec<(Option<i64>, Option<i64>, String)> {
    parse(text, LyricsSource::Sidecar).map_or_else(Vec::new, |sheet| {
        sheet.lines.into_iter().map(|line| (line.at_ms, line.end_ms, line.text)).collect()
    })
}

/// A line's word stamps as `(start, at)`, a start being a byte into the line's drawn text.
type Words = Vec<(usize, i64)>;

fn words(text: &str) -> Vec<Words> {
    paced(text).into_iter().map(|(_, words)| words).collect()
}

/// Each line's end beside its word stamps, which an end stamp and the span check move together.
fn paced(text: &str) -> Vec<(Option<i64>, Words)> {
    parse(text, LyricsSource::Sidecar).map_or_else(Vec::new, |sheet| {
        sheet
            .lines
            .into_iter()
            .map(|line| (line.end_ms, line.words.iter().map(|w| (w.start, w.at_ms)).collect()))
            .collect()
    })
}

#[test]
fn a_stamp_with_no_fraction_is_whole_seconds() {
    assert_eq!(stamps("[01:00]a"), vec![Some(60_000)]);
    assert_eq!(stamps("[00:00]a"), vec![Some(0)], "the first instant is a stamp like any other");
}

#[test]
fn a_fraction_is_read_to_milliseconds_whatever_its_precision() {
    // The format is written with one, two or three digits and all three mean the same thing.
    assert_eq!(stamps("[00:01.5]a"), vec![Some(1_500)]);
    assert_eq!(stamps("[00:01.50]a"), vec![Some(1_500)]);
    assert_eq!(stamps("[00:01.500]a"), vec![Some(1_500)]);
}

#[test]
fn a_fourth_fraction_digit_is_dropped_rather_than_refusing_the_stamp() {
    assert_eq!(stamps("[00:01.4567]a"), vec![Some(1_456)]);
}

#[test]
fn a_comma_is_a_decimal_point() {
    assert_eq!(stamps("[00:01,25]a"), vec![Some(1_250)]);
}

#[test]
fn minutes_are_not_capped_at_an_hour() {
    // A long recording is one file, and `mm` past 59 is how the format spells it.
    assert_eq!(stamps("[75:30.00]a"), vec![Some(4_530_000)]);
}

#[test]
fn a_signed_field_is_not_a_stamp() {
    // `i64::from_str` would take these; a negative minute is a position nothing could seek to,
    // so the line falls through to the untimed side rather than becoming a stamp.
    assert_eq!(stamps("[-1:00]a"), vec![None]);
    assert_eq!(stamps("[+5:00]a"), vec![None]);
}

#[test]
fn several_stamps_on_one_line_repeat_it_in_time_order() {
    // How the format spells a repeated chorus. Taking only the first loses the second half of
    // the song, silently.
    assert_eq!(
        drawn("[00:45.10][00:21.10]chorus"),
        vec![(Some(21_100), "chorus".to_owned()), (Some(45_100), "chorus".to_owned())]
    );
}

#[test]
fn two_lines_against_one_stamp_keep_the_order_they_were_written_in() {
    // The sort has to be stable, or a couplet reads backwards.
    assert_eq!(
        drawn("[00:01.00]first\n[00:01.00]second"),
        vec![(Some(1_000), "first".to_owned()), (Some(1_000), "second".to_owned())]
    );
}

#[test]
fn a_positive_offset_moves_the_sheet_earlier() {
    // The format's own wording reads both ways and the two readings are not equally wrong:
    // inverted, this doubles the error on exactly the sheets whose author noticed a drift.
    assert_eq!(stamps("[offset:+500]\n[00:10.00]a"), vec![Some(9_500)]);
}

#[test]
fn a_negative_offset_moves_the_sheet_later() {
    assert_eq!(stamps("[offset:-500]\n[00:10.00]a"), vec![Some(10_500)]);
}

#[test]
fn an_offset_past_the_first_stamp_clamps_at_the_start() {
    assert_eq!(stamps("[offset:+9000]\n[00:01.00]a"), vec![Some(0)]);
}

#[test]
fn word_stamps_are_taken_out_of_the_text_and_their_words_kept() {
    assert_eq!(
        drawn("[00:01.00]<00:01.00>one <00:01.50>two"),
        vec![(Some(1_000), "one two".to_owned())]
    );
}

#[test]
fn an_angle_bracket_that_is_not_a_stamp_stays_in_the_line() {
    assert_eq!(drawn("[00:01.00]i <3 you"), vec![(Some(1_000), "i <3 you".to_owned())]);
}

#[test]
fn a_seam_keeps_one_of_its_own_spaces_where_it_has_one_either_side() {
    // The line #177 reported.
    assert_eq!(
        drawn("[00:12.50] <00:12.50> Hello <00:12.80> world"),
        vec![(Some(12_500), "Hello world".to_owned())]
    );
}

#[test]
fn a_stamp_inside_a_word_joins_its_syllables() {
    assert_eq!(
        drawn("[00:01.00]<00:01.00>Hel<00:01.20>lo"),
        vec![(Some(1_000), "Hello".to_owned())]
    );
}

#[test]
fn a_full_width_space_at_a_seam_is_the_space_kept() {
    assert_eq!(
        drawn("[00:01.00]<00:01.00>一\u{3000}<00:02.00>二"),
        vec![(Some(1_000), "一\u{3000}二".to_owned())]
    );
}

#[test]
fn each_word_keeps_its_first_byte_in_the_drawn_text_and_its_time() {
    assert_eq!(words("[00:01.00]<00:01.00>one <00:01.50>two"), vec![vec![(0, 1_000), (4, 1_500)]]);
}

#[test]
fn a_word_s_start_counts_bytes_rather_than_characters() {
    // The panel cuts the text at it, where a character count lights the wrong syllables.
    assert_eq!(
        words("[00:01.00]<00:01.00>안녕 <00:01.50>세계"),
        vec![vec![(0, 1_000), (7, 1_500)]]
    );
}

#[test]
fn the_first_word_starts_past_any_text_written_ahead_of_its_stamp() {
    assert_eq!(words("[00:01.00]Oh <00:01.50>yeah"), vec![vec![(3, 1_500)]]);
}

#[test]
fn an_offset_moves_the_words_with_their_line() {
    assert_eq!(
        words("[offset:+500]\n[00:10.00]<00:10.00>a <00:10.50>b"),
        vec![vec![(0, 9_500), (2, 10_000)]]
    );
}

#[test]
fn a_line_written_under_two_stamps_lights_its_words_on_both() {
    assert_eq!(
        words("[00:10.00][00:30.00]<00:10.00>la <00:10.50>la"),
        vec![vec![(0, 10_000), (3, 10_500)], vec![(0, 30_000), (3, 30_500)]]
    );
}

#[test]
fn words_timed_against_a_later_repeat_light_on_the_first_too() {
    // Each copy keeps the gaps from the stamp the words were written against, whichever it is.
    assert_eq!(
        words("[00:10.00][00:30.00]<00:30.00>la <00:30.50>la"),
        vec![vec![(0, 10_000), (3, 10_500)], vec![(0, 30_000), (3, 30_500)]]
    );
}

#[test]
fn a_line_whose_words_come_before_its_only_stamp_lights_whole() {
    // Refused rather than shifted onto the stamp: nothing says which of the two is wrong.
    assert_eq!(words("[00:01.00]<00:00.999>a <00:01.50>b"), vec![Vec::new()]);
}

#[test]
fn a_stamp_after_the_last_word_is_the_line_s_end_rather_than_a_word() {
    assert_eq!(
        paced("[00:01.00]<00:01.00>one <00:01.50>two <00:02.00>\n[00:20.00]next"),
        vec![(Some(2_000), vec![(0, 1_000), (4, 1_500)]), (None, Vec::new())]
    );
}

#[test]
fn an_end_stamp_outranks_the_blank_stamp_after_it() {
    // The blank would push the notes of the break back to 10 s, after the singing stopped at 2.
    assert_eq!(
        folded("[00:01.00]<00:01.00>a <00:02.00>\n[00:10.00]\n[00:20.00]b"),
        vec![(Some(1_000), Some(2_000), "a".to_owned()), (Some(20_000), None, "b".to_owned())]
    );
}

#[test]
fn an_end_stamp_moves_with_the_copy_it_ends() {
    assert_eq!(
        folded("[00:10.00][00:30.00]<00:10.00>la <00:11.00>"),
        vec![
            (Some(10_000), Some(11_000), "la".to_owned()),
            (Some(30_000), Some(31_000), "la".to_owned()),
        ]
    );
}

#[test]
fn a_stamp_inside_the_gloss_is_no_word_and_breaks_no_order() {
    // 1.20 runs backwards from 1.50; read as a word, it would light the whole line at once.
    assert_eq!(
        words("[00:01.00]<00:01.00>one <00:01.50>two^<00:01.20>uno dos"),
        vec![vec![(0, 1_000), (4, 1_500)]]
    );
}

#[test]
fn a_stamp_inside_the_gloss_is_taken_out_of_its_text() {
    assert_eq!(
        glossed("[00:01.00]<00:01.00>one <00:01.50>two^<00:01.20>uno dos"),
        vec![("one two".to_owned(), Some("uno dos".to_owned()))]
    );
}

#[test]
fn words_out_of_order_light_their_line_whole() {
    assert_eq!(words("[00:01.00]<00:01.00>a <00:01.50>b <00:01.20>c"), vec![Vec::new()]);
}

#[test]
fn two_words_on_one_instant_are_in_order() {
    assert_eq!(words("[00:01.00]<00:01.00>a <00:01.00>b"), vec![vec![(0, 1_000), (2, 1_000)]]);
}

#[test]
fn an_end_stamp_before_the_last_word_drops_the_words_and_the_end_together() {
    assert_eq!(
        paced("[00:01.00]<00:01.00>a <00:02.00>b <00:01.50>\n[00:20.00]c"),
        vec![(None, Vec::new()), (None, Vec::new())]
    );
}

#[test]
fn a_line_s_words_may_run_up_to_the_next_line_s_stamp_and_not_past_it() {
    assert_eq!(
        words("[00:01.00]<00:01.00>a <00:03.00>b\n[00:03.00]c"),
        vec![vec![(0, 1_000), (2, 3_000)], Vec::new()],
        "a word on the next stamp"
    );
    assert_eq!(
        words("[00:01.00]<00:01.00>a <00:03.001>b\n[00:03.00]c"),
        vec![Vec::new(), Vec::new()],
        "a millisecond past it"
    );
}

#[test]
fn an_end_stamp_past_the_next_line_s_stamp_lights_its_line_whole() {
    assert_eq!(
        paced("[00:01.00]<00:01.00>a <00:04.00>\n[00:03.00]b"),
        vec![(None, Vec::new()), (None, Vec::new())]
    );
}

#[test]
fn a_blank_stamp_bounds_the_words_before_it_as_a_line_would() {
    // Lit whole, the line then ends at the blank as any line does.
    assert_eq!(
        paced("[00:01.00]<00:01.00>a <00:05.00>b\n[00:03.00]\n[00:10.00]c"),
        vec![(Some(3_000), Vec::new()), (None, Vec::new())]
    );
}

#[test]
fn a_line_sharing_its_stamp_with_the_next_is_bounded_by_the_stamp_after_both() {
    // A couplet under one stamp: bounded by its neighbour's, the first line would light whole.
    assert_eq!(
        words("[00:01.00]<00:01.00>a <00:02.00>b\n[00:01.00]c\n[00:05.00]d"),
        vec![vec![(0, 1_000), (2, 2_000)], Vec::new(), Vec::new()]
    );
}

#[test]
fn the_last_millisecond_an_i64_holds_is_a_stamp_and_the_next_is_not() {
    assert_eq!(stamps("[0:9223372036854775.807]a"), vec![Some(i64::MAX)]);
    assert_eq!(stamps("[0:9223372036854775.808]a"), vec![None]);
}

#[test]
fn a_stamp_whose_minutes_or_seconds_overflow_milliseconds_is_not_a_stamp() {
    // Text from a tag or a download: unchecked, the overflow panics a debug build and wraps a
    // release one.
    assert_eq!(stamps("[200000000000000000:00]a"), vec![None], "minutes");
    assert_eq!(stamps("[0:9223372036854776]a"), vec![None], "seconds");
}

#[test]
fn a_sheet_broken_only_by_carriage_returns_reads_line_by_line() {
    assert_eq!(
        drawn("[ti:x]\r[00:01.00]one\r[00:03.00]two\r"),
        vec![(Some(1_000), "one".to_owned()), (Some(3_000), "two".to_owned())]
    );
}

#[test]
fn a_sheet_broken_by_carriage_returns_keeps_its_offset() {
    assert_eq!(stamps("[offset:+500]\r[00:10.00]a"), vec![Some(9_500)]);
}

#[test]
fn identification_tags_are_dropped_rather_than_sung() {
    assert_eq!(
        drawn("[ti:Song]\n[ar:Band]\n[al:Record]\n[by:Someone]\n[00:01.00]words"),
        vec![(Some(1_000), "words".to_owned())]
    );
}

#[test]
fn a_timed_sheet_keeps_only_its_timed_lines() {
    // What makes "a sheet is timed or it is not" true downstream rather than hoped for.
    assert_eq!(
        drawn("[00:01.00]a\nstray\n[00:02.00]b"),
        vec![(Some(1_000), "a".to_owned()), (Some(2_000), "b".to_owned())]
    );
}

#[test]
fn a_timed_line_with_no_words_ends_the_line_above_it() {
    // Drawn as a row of its own it is the blank the panel centres on for the length of a solo.
    assert_eq!(
        folded("[00:01.00]words\n[00:20.00]"),
        vec![(Some(1_000), Some(20_000), "words".to_owned())]
    );
}

#[test]
fn an_untimed_sheet_keeps_its_markers_and_its_blank_lines() {
    // A plain sheet spaces its verses with blank lines, and `[Chorus]` carries no colon and no
    // known key, so nothing mistakes it for metadata.
    assert_eq!(
        drawn("[Chorus]\n\nhello"),
        vec![(None, "[Chorus]".to_owned()), (None, String::new()), (None, "hello".to_owned()),]
    );
}

#[test]
fn a_sheet_with_nothing_to_draw_is_no_sheet() {
    assert!(parse("", LyricsSource::Sidecar).is_none(), "an empty file");
    assert!(parse("   \n \n", LyricsSource::Sidecar).is_none(), "whitespace only");
    assert!(parse("[ti:Song]\n[ar:Band]\n", LyricsSource::Sidecar).is_none(), "metadata only");
}

#[test]
fn a_byte_order_mark_does_not_swallow_the_first_stamp() {
    assert_eq!(stamps("\u{feff}[00:01.00]a"), vec![Some(1_000)]);
}

#[test]
fn carriage_returns_do_not_reach_the_drawn_text() {
    assert_eq!(drawn("[00:01.00]a\r\n[00:02.00]b")[0].1, "a");
}

#[test]
fn the_source_is_carried_through_to_the_sheet() {
    let sheet = parse("[00:01.00]a", LyricsSource::Online);
    assert_eq!(sheet.map(|s| s.source), Some(LyricsSource::Online));
}

#[test]
fn a_sheet_knows_whether_it_can_be_followed() {
    let timed = parse("[00:01.00]a", LyricsSource::Tag);
    let plain = parse("just words", LyricsSource::Tag);
    assert_eq!(timed.map(|s| s.is_synced()), Some(true));
    assert_eq!(plain.map(|s| s.is_synced()), Some(false));
}

/// Each line with the gloss drawn under it, the pair being what a bilingual sheet carries.
fn glossed(text: &str) -> Vec<(String, Option<String>)> {
    parse(text, LyricsSource::Sidecar).map_or_else(Vec::new, |sheet| {
        sheet.lines.into_iter().map(|line| (line.text, line.translation)).collect()
    })
}

#[test]
fn a_caret_separates_the_words_from_the_gloss_under_them() {
    assert_eq!(
        glossed("[00:01.00]sung line^translated line"),
        vec![("sung line".to_owned(), Some("translated line".to_owned()))]
    );
}

#[test]
fn a_caret_with_nothing_on_one_side_is_a_caret_rather_than_a_separator() {
    // A sheet that simply contains one keeps it. Split anyway, a line opening on a caret would
    // lose its words and a line ending on one would gain an empty second row.
    assert_eq!(glossed("[00:01.00]^only a gloss"), vec![("^only a gloss".to_owned(), None)]);
    assert_eq!(glossed("[00:01.00]only the words^"), vec![("only the words^".to_owned(), None)]);
    assert_eq!(glossed("[00:01.00]words^   "), vec![("words^".to_owned(), None)], "whitespace");
}

#[test]
fn the_first_caret_is_the_separator_and_the_rest_ride_in_the_gloss() {
    assert_eq!(
        glossed("[00:01.00]words^gloss^more"),
        vec![("words".to_owned(), Some("gloss^more".to_owned()))]
    );
}

#[test]
fn the_two_halves_are_trimmed_apart() {
    assert_eq!(
        glossed("[00:01.00]words  ^  gloss"),
        vec![("words".to_owned(), Some("gloss".to_owned()))]
    );
}

#[test]
fn a_gloss_is_repeated_with_the_line_a_second_stamp_repeats() {
    // The chorus case. Carried on the first copy alone, the second time round the panel draws the
    // words with nothing under them.
    assert_eq!(
        glossed("[00:45.10][00:21.10]words^gloss"),
        vec![
            ("words".to_owned(), Some("gloss".to_owned())),
            ("words".to_owned(), Some("gloss".to_owned())),
        ]
    );
}

#[test]
fn an_untimed_sheet_carries_its_glosses_too() {
    assert_eq!(glossed("words^gloss"), vec![("words".to_owned(), Some("gloss".to_owned()))]);
}

#[test]
fn a_sheet_is_timed_when_any_line_carries_a_stamp() {
    // The text side of the resolution order asks this of raw text, where the sheet side asks a
    // parsed `Lyrics`. They are different questions and this is the one a promotion is decided on.
    assert!(is_timed("[00:01.00]a"));
    assert!(is_timed("stray words\n[00:01.00]a"), "one stamped line is enough");
    assert!(is_timed("stray words\r[00:01.00]a"), "behind a lone carriage return too");
    assert!(!is_timed("just words\nand more"));
    assert!(!is_timed(""));
    assert!(!is_timed("[ti:Song]"), "an identification tag is not a stamp");
}
