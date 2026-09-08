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
