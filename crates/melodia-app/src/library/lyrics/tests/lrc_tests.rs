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
        vec![
            (Some(21_100), "chorus".to_owned()),
            (Some(45_100), "chorus".to_owned())
        ]
    );
}

#[test]
fn two_lines_against_one_stamp_keep_the_order_they_were_written_in() {
    // The sort has to be stable, or a couplet reads backwards.
    assert_eq!(
        drawn("[00:01.00]first\n[00:01.00]second"),
        vec![
            (Some(1_000), "first".to_owned()),
            (Some(1_000), "second".to_owned())
        ]
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
fn word_stamps_are_dropped_and_their_words_kept() {
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
fn a_timed_line_with_no_words_survives_as_an_instrumental_gap() {
    assert_eq!(
        drawn("[00:01.00]words\n[00:20.00]"),
        vec![
            (Some(1_000), "words".to_owned()),
            (Some(20_000), String::new())
        ]
    );
}

#[test]
fn an_untimed_sheet_keeps_its_markers_and_its_blank_lines() {
    // A plain sheet spaces its verses with blank lines, and `[Chorus]` carries no colon and no
    // known key, so nothing mistakes it for metadata.
    assert_eq!(
        drawn("[Chorus]\n\nhello"),
        vec![
            (None, "[Chorus]".to_owned()),
            (None, String::new()),
            (None, "hello".to_owned()),
        ]
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
