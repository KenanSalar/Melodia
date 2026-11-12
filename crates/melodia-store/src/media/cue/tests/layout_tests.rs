//! Where each track of a sheet starts and ends, in which file, and when the sheet splits a file at
//! all. Times are spelled as the sheet spells them; the frame counts beside them are worked at 75 to
//! the second.

use std::path::Path;

use super::*;
use crate::media::cue::parse::{Files, parse};

/// A row as `(number, start, end)` in cue frames.
type Row = (u8, i64, Option<i64>);

fn lay_out_lines(lines: &[&str]) -> Vec<FileLayout> {
    let sheet = parse(&lines.join("\n"), Path::new("album.cue"), Files::Named);
    lay_out(&sheet, Path::new("album.cue"))
}

fn rows_of(layout: &FileLayout) -> Vec<Row> {
    layout
        .rows
        .iter()
        .map(|row| {
            let span = row.span;
            (row.track.number(), i64::from(span.start()), span.end().map(i64::from))
        })
        .collect()
}

fn laid_out(lines: &[&str]) -> Vec<(usize, Vec<Row>)> {
    lay_out_lines(lines).iter().map(|layout| (layout.file, rows_of(layout))).collect()
}

const THREE_TRACKS: [&str; 7] = [
    "FILE \"album.wav\" WAVE",
    "TRACK 01 AUDIO",
    "INDEX 01 00:00:00",
    "TRACK 02 AUDIO",
    "INDEX 01 03:00:00",
    "TRACK 03 AUDIO",
    "INDEX 01 07:00:00",
];

#[test]
fn each_track_ends_where_the_next_starts_and_the_last_plays_to_the_end() {
    assert_eq!(
        laid_out(&THREE_TRACKS),
        [(0, vec![(1, 0, Some(13_500)), (2, 13_500, Some(31_500)), (3, 31_500, None)])]
    );
}

#[test]
fn a_pregap_plays_at_the_end_of_the_track_before() {
    let rows = laid_out(&[
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 00 02:58:00",
        "INDEX 01 03:00:00",
    ]);

    assert_eq!(rows, [(0, vec![(1, 0, Some(13_500)), (2, 13_500, None)])]);
}

#[test]
fn audio_ahead_of_track_one_is_a_hidden_track_from_one_second() {
    let rows = [
        ("00:00:74", None),
        ("00:01:00", Some((0, 0, Some(75)))),
        ("00:01:01", Some((0, 0, Some(76)))),
    ];
    for (start, hidden) in rows {
        let layouts = laid_out(&[
            "FILE \"album.wav\" WAVE",
            "TRACK 01 AUDIO",
            "INDEX 00 00:00:00",
            &format!("INDEX 01 {start}"),
            "TRACK 02 AUDIO",
            "INDEX 01 04:00:00",
        ]);
        let first = layouts.first().and_then(|(_, rows)| rows.first().copied());
        let found = first.filter(|&(number, _, _)| number == 0);
        assert_eq!(found, hidden, "track 1 at {start}");
    }
}

#[test]
fn the_hidden_track_comes_first_and_counts_toward_the_claim_but_not_the_total() {
    let layouts = lay_out_lines(&[
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 00 00:00:00",
        "INDEX 01 00:30:00",
    ]);

    let shape: Vec<(Vec<Row>, bool, usize)> = layouts
        .iter()
        .map(|layout| (rows_of(layout), layout.claims(), layout.track_total()))
        .collect();

    assert_eq!(shape, [(vec![(0, 0, Some(2_250)), (1, 2_250, None)], true, 1)]);
}

/// Track 1's `INDEX 00` sitting in the file before its own is the per-track rip's pregap, not
/// audio hidden ahead of it.
#[test]
fn no_hidden_track_spans_two_files() {
    let rows = laid_out(&[
        "FILE \"00.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 00 00:00:00",
        "FILE \"01.wav\" WAVE",
        "INDEX 01 00:05:00",
    ]);

    assert_eq!(rows, [(1, vec![(1, 375, None)])]);
}

#[test]
fn a_track_belongs_to_the_file_its_index_01_sits_under() {
    let layouts = lay_out_lines(&[
        "FILE \"01.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 00 03:58:00",
        "FILE \"02.wav\" WAVE",
        "INDEX 01 00:00:00",
    ]);

    let shape: Vec<(usize, Vec<Row>, bool)> =
        layouts.iter().map(|layout| (layout.file, rows_of(layout), layout.claims())).collect();

    assert_eq!(shape, [(0, vec![(1, 0, None)], false), (1, vec![(2, 0, None)], false)]);
}

#[test]
fn a_data_track_takes_no_row_but_ends_the_one_before() {
    let rows = laid_out(&[
        "FILE \"album.bin\" BINARY",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 MODE1/2352",
        "INDEX 01 03:00:00",
        "TRACK 03 AUDIO",
        "INDEX 01 07:00:00",
    ]);

    assert_eq!(rows, [(0, vec![(1, 0, Some(13_500)), (3, 31_500, None)])]);
}

#[test]
fn a_track_with_no_index_01_takes_no_row_and_ends_nothing() {
    let rows = laid_out(&[
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 00 03:00:00",
        "TRACK 03 AUDIO",
        "INDEX 01 07:00:00",
    ]);

    assert_eq!(rows, [(0, vec![(1, 0, Some(31_500)), (3, 31_500, None)])]);
}

#[test]
fn a_track_ending_where_it_starts_or_before_is_dropped_alone() {
    let rows = [("05:00:00", 22_500), ("04:00:00", 18_000)];
    for (third, third_start) in rows {
        let layouts = laid_out(&[
            "FILE \"album.wav\" WAVE",
            "TRACK 01 AUDIO",
            "INDEX 01 00:00:00",
            "TRACK 02 AUDIO",
            "INDEX 01 05:00:00",
            "TRACK 03 AUDIO",
            &format!("INDEX 01 {third}"),
        ]);
        let expected = vec![(0, vec![(1, 0, Some(22_500)), (3, third_start, None)])];
        assert_eq!(layouts, expected, "track 3 at {third}");
    }
}

#[test]
fn a_track_number_placed_twice_keeps_its_first_place() {
    let rows = laid_out(&[
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 01 03:00:00",
        "TRACK 02 AUDIO",
        "INDEX 01 07:00:00",
    ]);

    assert_eq!(rows, [(0, vec![(1, 0, Some(13_500)), (2, 13_500, Some(31_500))])]);
}

#[test]
fn a_file_is_claimed_from_two_rows() {
    let one = lay_out_lines(&THREE_TRACKS[..3]);
    let two = lay_out_lines(&THREE_TRACKS[..5]);

    let claims =
        |layouts: &[FileLayout]| layouts.iter().map(FileLayout::claims).collect::<Vec<_>>();

    assert_eq!((claims(&one), claims(&two)), (vec![false], vec![true]));
}

#[test]
fn each_row_reads_its_fields_from_its_own_sheet_track() {
    let layouts = lay_out_lines(&[
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 00 00:00:00",
        "INDEX 01 00:02:00",
        "TRACK 02 MODE1/2352",
        "INDEX 01 03:00:00",
        "TRACK 03 AUDIO",
        "INDEX 01 07:00:00",
    ]);

    let sources: Vec<Option<usize>> =
        layouts.iter().flat_map(|layout| &layout.rows).map(|row| row.source).collect();

    assert_eq!(sources, [None, Some(0), Some(2)]);
}
