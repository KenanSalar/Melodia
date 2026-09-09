//! Which line the panel calls sung, and how tall it draws a row.
//!
//! Both are silent when wrong: an off-by-one in the search highlights the wrong line for the whole
//! song, and a row height that disagrees with the layout drifts the scroll further down a sheet.

use super::*;

fn timed(stamps: &[i32]) -> Vec<Row> {
    stamps
        .iter()
        .map(|at| Row {
            kind: RowKind::Words,
            at_ms: Some(*at),
            text: "x".to_owned(),
            romanization: None,
            translation: None,
            lines: 1,
            romanization_lines: 0,
            translation_lines: 0,
        })
        .collect()
}

/// The width the panel reports at its clamp's midpoint, less the scrollbar lane.
const TYPICAL_WIDTH: f32 = 340.0;

/// The size `Lyrics.font-size` sets a line at.
const TYPICAL_SIZE: f32 = 16.0;

#[test]
fn nothing_is_sung_before_the_first_stamp() {
    let rows = timed(&[1_000, 5_000]);
    assert_eq!(sung_at(&rows, 0.0), None);
    assert_eq!(sung_at(&rows, 999.0), None, "the step below the boundary");
}

#[test]
fn a_line_is_sung_from_the_instant_its_stamp_arrives() {
    let rows = timed(&[1_000, 5_000]);
    assert_eq!(sung_at(&rows, 1_000.0), Some(0));
}

#[test]
fn a_line_holds_until_the_next_stamp() {
    let rows = timed(&[1_000, 5_000]);
    assert_eq!(sung_at(&rows, 4_999.0), Some(0));
    assert_eq!(sung_at(&rows, 5_000.0), Some(1));
}

#[test]
fn the_last_line_holds_to_the_end_of_the_track() {
    let rows = timed(&[1_000, 5_000]);
    assert_eq!(sung_at(&rows, 600_000.0), Some(1));
}

#[test]
fn a_repeated_stamp_sings_the_later_of_the_two_lines() {
    // A chorus written twice against one stamp: landing before the run would highlight a line the
    // sheet has already passed.
    let rows = timed(&[1_000, 1_000, 4_000]);
    assert_eq!(sung_at(&rows, 1_000.0), Some(1));
}

#[test]
fn an_untimed_sheet_sings_nothing() {
    let rows = vec![Row {
        kind: RowKind::Words,
        at_ms: None,
        text: "a".to_owned(),
        romanization: None,
        translation: None,
        lines: 1,
        romanization_lines: 0,
        translation_lines: 0,
    }];
    assert_eq!(sung_at(&rows, 5_000.0), None);
}

#[test]
fn an_empty_sheet_sings_nothing() {
    assert_eq!(sung_at(&[], 5_000.0), None);
}

#[test]
fn a_short_line_takes_one_row() {
    assert_eq!(wrapped_lines("short line", TYPICAL_WIDTH, TYPICAL_SIZE), 1);
}

#[test]
fn a_blank_line_still_takes_a_row() {
    // A plain sheet spaces its verses with them, so collapsing one to nothing loses the spacing.
    // An interlude row is blank by construction too, so this floor is what gives it its height.
    assert_eq!(wrapped_lines("", TYPICAL_WIDTH, TYPICAL_SIZE), 1);
    assert_eq!(wrapped_lines("   ", TYPICAL_WIDTH, TYPICAL_SIZE), 1);
}

#[test]
fn a_line_past_one_row_takes_two() {
    assert_eq!(wrapped_lines(&"x".repeat(70), TYPICAL_WIDTH, TYPICAL_SIZE), 2);
}

#[test]
fn a_line_long_enough_to_be_a_paragraph_is_capped() {
    // Past the cap the panel would scroll more than it shows.
    assert_eq!(wrapped_lines(&"x".repeat(4_000), TYPICAL_WIDTH, TYPICAL_SIZE), MAX_WRAPPED_LINES);
}

#[test]
fn a_width_nobody_has_reported_yet_is_survivable() {
    // The panel reports on its own timer, so the first frame estimates against nothing.
    assert_eq!(wrapped_lines("anything", 0.0, TYPICAL_SIZE), 1);
    assert_eq!(wrapped_lines("anything", -5.0, TYPICAL_SIZE), 1);
}

/// A run long enough that the three width classes land on three different row counts, so
/// merging any two of them is a failure rather than a coincidence.
const CLASS_RUN: usize = 55;

#[test]
fn a_line_is_measured_by_its_letters_rather_than_by_its_length() {
    // One averaged width put all three of these on the same row count, which is what over-charged
    // ordinary prose by a sixth and under-charged a line of `m`s by as much again.
    assert_eq!(wrapped_lines(&"l".repeat(CLASS_RUN), TYPICAL_WIDTH, TYPICAL_SIZE), 1, "narrow");
    assert_eq!(wrapped_lines(&"o".repeat(CLASS_RUN), TYPICAL_WIDTH, TYPICAL_SIZE), 2, "ordinary");
    assert_eq!(wrapped_lines(&"m".repeat(CLASS_RUN), TYPICAL_WIDTH, TYPICAL_SIZE), 3, "wide");
}

#[test]
fn a_hangul_syllable_with_no_final_consonant_is_charged_the_two_ems_it_is_drawn_at() {
    // Slint sets those as two loose jamo, so a line of them is twice as wide as its syllable
    // count says; charging both kinds a square em wrapped a Korean line the panel had no row for.
    assert_eq!(wrapped_lines(&"안".repeat(12), TYPICAL_WIDTH, TYPICAL_SIZE), 1, "final consonant");
    assert_eq!(wrapped_lines(&"아".repeat(12), TYPICAL_WIDTH, TYPICAL_SIZE), 2, "open syllable");
}

/// A stamped line, with `end_ms` where the sheet says when it stops.
fn line(at_ms: i64, end_ms: Option<i64>, text: &str) -> LyricLine {
    LyricLine {
        at_ms: Some(at_ms),
        end_ms,
        text: text.to_owned(),
        romanization: None,
        translation: None,
    }
}

fn sheet(lines: Vec<LyricLine>) -> Option<Sheet> {
    Sheet::new(lines, LyricsSource::Sidecar)
}

/// Each row as either words or the gap they sit around, which is what the two are told apart by.
fn gaps(rows: &[Row]) -> Vec<Option<(i32, i32)>> {
    rows.iter()
        .map(|row| match row.kind {
            RowKind::Words => None,
            RowKind::Interlude { until_ms } => Some((row.at_ms.unwrap_or(-1), until_ms)),
        })
        .collect()
}

/// The rows a sheet is drawn as, or nothing where it was not a sheet at all.
fn drawn_rows(lines: Vec<LyricLine>) -> Vec<Row> {
    sheet(lines).as_ref().map(rows_for).unwrap_or_default()
}

#[test]
fn an_untimed_sheet_is_drawn_without_gaps() {
    // Nothing to measure a rest against, so every row is words and the panel scrolls by hand.
    let rows = drawn_rows(vec![
        LyricLine {
            at_ms: None,
            end_ms: None,
            text: "a".to_owned(),
            romanization: None,
            translation: None,
        },
        LyricLine {
            at_ms: None,
            end_ms: None,
            text: "b".to_owned(),
            romanization: None,
            translation: None,
        },
    ]);

    assert_eq!(gaps(&rows), vec![None, None]);
}

#[test]
fn the_run_in_is_drawn_at_its_own_floor() {
    // Lower than a mid-song rest, because a gap with no line above it costs no scrolling: the
    // panel mounts on it and leaves it once.
    let on_it = i64::from(INTRO_MS);
    assert_eq!(
        gaps(&drawn_rows(vec![line(on_it, None, "first")])),
        vec![Some((0, INTRO_MS)), None],
        "a run-in exactly on the floor is drawn"
    );
    assert_eq!(
        gaps(&drawn_rows(vec![line(on_it - 1, None, "first")])),
        vec![None],
        "and the step under it is not"
    );
}

#[test]
fn a_rest_between_two_lines_is_drawn_at_the_longer_floor() {
    // A shorter break would put three scroll targets inside one breath, which reads as the panel
    // losing its place rather than as the song resting.
    let sung_until = 1_000;
    let on_it = sung_until + i64::from(INTERLUDE_MS);
    assert_eq!(
        gaps(&drawn_rows(vec![
            line(0, Some(sung_until), "first"),
            line(on_it, None, "second"),
        ])),
        vec![None, Some((1_000, 6_000)), None],
        "a rest exactly on the floor is drawn"
    );
    assert_eq!(
        gaps(&drawn_rows(vec![
            line(0, Some(sung_until), "first"),
            line(on_it - 1, None, "second"),
        ])),
        vec![None, None],
        "and the step under it is not"
    );
}

#[test]
fn a_line_the_sheet_never_closes_cannot_open_a_rest_under_it() {
    // An absent end stamp is the sheet declining to say, and a long line and a long silence are
    // indistinguishable without one. Guessing draws notes over words still being sung.
    let rows = drawn_rows(vec![line(0, None, "first"), line(600_000, None, "second")]);

    assert_eq!(gaps(&rows), vec![None, None]);
}

#[test]
fn an_interlude_carries_both_ends_of_the_span_it_fills() {
    // Its own stamp is where the gap starts and the row has to know where it ends, the notes
    // filling across the distance between them.
    let rows = drawn_rows(vec![line(0, Some(1_000), "first"), line(10_000, None, "second")]);

    assert_eq!(gaps(&rows), vec![None, Some((1_000, 10_000)), None]);
}

#[test]
fn a_sheet_longer_than_the_panel_draws_is_cut_to_the_cap() {
    // The panel does not virtualize, so this is the bound that makes a plain `for` affordable.
    // Nothing about a lyrics tag is length-limited and it comes from outside.
    let overlong = (0..MAX_ROWS + 100)
        .map(|n| {
            let at = i64::try_from(n).unwrap_or(i64::MAX) * 100;
            line(at, Some(at), "words")
        })
        .collect();

    assert_eq!(drawn_rows(overlong).len(), MAX_ROWS);
}

#[test]
fn a_stamp_past_what_a_row_can_hold_saturates_at_the_top() {
    // Wrapped instead, a malformed stamp sorts to the front and the panel opens on the last line
    // of the song.
    assert_eq!(millis(1_000), 1_000);
    assert_eq!(millis(i64::from(i32::MAX) + 1), i32::MAX);
}

/// A type scale whose parts are all different, so a row height that reached the wrong one is a
/// wrong number rather than a coincidence.
fn metrics() -> Metrics {
    Metrics {
        font_size: 16.0,
        line_height: 20.0,
        romanization_font_size: 12.0,
        romanization_line_height: 14.0,
        romanization_gap: 3.0,
        translation_font_size: 13.0,
        translation_line_height: 15.0,
        translation_gap: 5.0,
        row_gap: 8.0,
        row_pad_y: 2.0,
    }
}

/// Two rows of one line each, laid out with a gap between them that belongs to neither.
const FIRST_TOP: f32 = 0.0;
const SECOND_TOP: f32 = 32.0;

/// `metrics()`'s height for a one-line row with nothing under it: the padding twice over the line.
const PLAIN_ROW_H: f32 = 24.0;

/// Heights are exact sums of the scale above, so an equality is honest here.
const TOLERANCE: f32 = 0.001;

#[test]
fn a_row_with_nothing_under_it_is_its_words_and_its_padding() {
    // The gap between rows is the layout's spacing and belongs to neither of the two it separates,
    // so counting it here would drift the offset table a row further down every line.
    let rows = timed(&[0]);
    let Some(row) = rows.first() else {
        return;
    };
    assert!((metrics().row_height(row) - PLAIN_ROW_H).abs() < TOLERANCE);
}

#[test]
fn each_block_under_the_words_charges_the_gap_above_it() {
    let mut row = Row {
        kind: RowKind::Words,
        at_ms: Some(0),
        text: "x".to_owned(),
        romanization: Some("x".to_owned()),
        translation: None,
        lines: 1,
        romanization_lines: 1,
        translation_lines: 0,
    };
    let romanized = PLAIN_ROW_H + 3.0 + 14.0;
    assert!((metrics().row_height(&row) - romanized).abs() < TOLERANCE, "the romanization");

    row.translation = Some("x".to_owned());
    row.translation_lines = 2;
    let both = romanized + 5.0 + 15.0 * 2.0;
    assert!((metrics().row_height(&row) - both).abs() < TOLERANCE, "and the gloss beneath it");
}

#[test]
fn a_block_a_row_does_not_have_charges_nothing() {
    // Zero rows of gloss must cost zero, gap included. Charging the gap regardless puts every row
    // in an unglossed sheet out by it.
    let plain = timed(&[0]);
    let Some(plain) = plain.first() else {
        return;
    };
    assert!((metrics().row_height(plain) - PLAIN_ROW_H).abs() < TOLERANCE);
}

#[test]
fn a_point_above_the_sheet_is_no_row() {
    let rows = timed(&[0, 1_000]);
    let offsets = [FIRST_TOP, SECOND_TOP];

    assert_eq!(row_at(&offsets, &rows, metrics(), -1.0), None);
}

#[test]
fn a_point_on_a_rows_top_edge_selects_that_row() {
    let rows = timed(&[0, 1_000]);
    let offsets = [FIRST_TOP, SECOND_TOP];

    assert_eq!(row_at(&offsets, &rows, metrics(), SECOND_TOP), Some(1));
    assert_eq!(row_at(&offsets, &rows, metrics(), SECOND_TOP - 0.001), Some(0), "the step above");
}

#[test]
fn the_gap_between_two_rows_belongs_to_the_row_above() {
    // The gaps are wide enough to be missed into, and a click a few pixels wide of a line plainly
    // meant that line.
    let rows = timed(&[0, 1_000]);
    let offsets = [FIRST_TOP, SECOND_TOP];

    assert_eq!(row_at(&offsets, &rows, metrics(), PLAIN_ROW_H + 1.0), Some(0));
}

#[test]
fn the_whitespace_below_the_last_row_is_no_row() {
    // There is no line down there to have meant, and it is most of what gets clicked by accident.
    let rows = timed(&[0, 1_000]);
    let offsets = [FIRST_TOP, SECOND_TOP];
    let bottom = SECOND_TOP + PLAIN_ROW_H;

    assert_eq!(row_at(&offsets, &rows, metrics(), bottom), Some(1), "its own bottom edge");
    assert_eq!(row_at(&offsets, &rows, metrics(), bottom + 0.001), None, "the step past it");
}

#[test]
fn an_empty_sheet_has_no_row_to_click() {
    assert_eq!(row_at(&[], &[], metrics(), 0.0), None);
}

#[test]
fn a_row_of_words_has_nothing_to_fill() {
    let rows = timed(&[0]);
    let Some(row) = rows.first() else {
        return;
    };
    assert!((interlude_progress(row, 500.0) - 0.0).abs() < TOLERANCE);
}

#[test]
fn a_gap_fills_across_the_span_between_its_two_stamps() {
    let row = Row::interlude(1_000, 3_000);

    assert!((interlude_progress(&row, 1_000.0) - 0.0).abs() < TOLERANCE, "at the start");
    assert!((interlude_progress(&row, 2_000.0) - 0.5).abs() < TOLERANCE, "halfway");
    assert!((interlude_progress(&row, 3_000.0) - 1.0).abs() < TOLERANCE, "at the end");
}

#[test]
fn a_position_outside_the_gap_is_clamped_rather_than_run_past() {
    // The panel is drawn from this directly, so an unclamped ratio paints the notes outside their
    // own row.
    let row = Row::interlude(1_000, 3_000);

    assert!((interlude_progress(&row, 0.0) - 0.0).abs() < TOLERANCE, "before it");
    assert!((interlude_progress(&row, 600_000.0) - 1.0).abs() < TOLERANCE, "long after it");
}

#[test]
fn a_gap_whose_stamps_disagree_is_drawn_full() {
    // Full rather than empty, so a sheet whose stamps run backwards leaves nothing filling on
    // screen for the rest of the song.
    let backwards = Row::interlude(3_000, 1_000);
    let instant = Row::interlude(1_000, 1_000);

    assert!((interlude_progress(&backwards, 2_000.0) - 1.0).abs() < TOLERANCE);
    assert!((interlude_progress(&instant, 1_000.0) - 1.0).abs() < TOLERANCE);
}

#[test]
fn a_capital_is_charged_more_than_a_lowercase_letter_and_less_than_a_wide_one() {
    // The one width class the wrapping cases do not reach, and a title-cased line is most of what
    // a chorus is written in.
    assert!((char_ems('A') - UPPERCASE_EMS).abs() < TOLERANCE);
    assert!(char_ems('A') > char_ems('o'), "wider than an ordinary lowercase letter");
    assert!(char_ems('A') < char_ems('m'), "narrower than the widest one");
}
