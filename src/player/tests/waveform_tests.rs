//! Tests for the visualizer's oscilloscope trace.

use super::*;

const RATE: u32 = 44_100;
/// A window generous enough for any rate these tests use, matching the ring the
/// production analyzer is sized to.
const WINDOW_CAP: usize = 16_384;

// --- helpers -----------------------------------------------------------------

/// Fill a buffer with a full-scale sine of `periods` whole cycles.
#[allow(
    clippy::cast_precision_loss,
    reason = "test buffers are a few thousand samples, which convert to f32 exactly"
)]
fn fill_sine(buf: &mut [f32], periods: f32) {
    let len = buf.len() as f32;
    for (i, sample) in buf.iter_mut().enumerate() {
        *sample = (2.0 * std::f32::consts::PI * periods * i as f32 / len).sin();
    }
}

/// Assert two values are equal to within a tight tolerance.
fn approx(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
}

/// The tallest half-height any column reaches.
fn peak(columns: &[Column]) -> f32 {
    columns
        .iter()
        .fold(0.0_f32, |m, c| m.max(c.max.abs()).max(c.min.abs()))
}

// --- find_trigger ------------------------------------------------------------

#[test]
fn the_trigger_lands_on_a_rising_crossing_of_a_sine() {
    let mut buf = vec![0.0; 1024];
    fill_sine(&mut buf, 8.0);

    let trigger = find_trigger(&buf, buf.len());

    // The sample at the trigger is at or just above zero, and the one before it
    // is below — that is what "rising crossing" means.
    assert!(buf[trigger] >= 0.0, "trigger sample {} is below zero", buf[trigger]);
    assert!(trigger > 0, "a sine starting at zero should not trigger on sample 0");
    assert!(buf[trigger - 1] < 0.0, "sample before the trigger should be negative");
}

#[test]
fn the_trigger_picks_the_most_recent_crossing() {
    let mut buf = vec![0.0; 1024];
    fill_sine(&mut buf, 8.0);

    // Eight periods means eight rising crossings; the chosen one must sit in the
    // last of them rather than the first.
    let trigger = find_trigger(&buf, buf.len());
    assert!(trigger > buf.len() * 3 / 4, "expected a late crossing, got {trigger}");
}

#[test]
fn the_trigger_search_stops_at_search_len() {
    let mut buf = vec![0.0; 1024];
    fill_sine(&mut buf, 8.0);

    let short = find_trigger(&buf, 256);
    assert!(short < 256, "trigger {short} escaped the search window");
}

#[test]
fn silence_has_no_trigger() {
    let buf = vec![0.0; 512];
    assert_eq!(find_trigger(&buf, buf.len()), 0);
}

#[test]
fn a_positive_dc_offset_has_no_trigger() {
    // Never dips below the hysteresis, so nothing ever arms.
    let buf = vec![0.5; 512];
    assert_eq!(find_trigger(&buf, buf.len()), 0);
}

#[test]
fn a_negative_dc_offset_has_no_trigger() {
    // Arms immediately but never crosses back up.
    let buf = vec![-0.5; 512];
    assert_eq!(find_trigger(&buf, buf.len()), 0);
}

#[test]
fn noise_inside_the_hysteresis_band_does_not_trigger() {
    // Alternating either side of zero, but never far enough below it to arm —
    // exactly the signal an unhysteretic trigger would chase every frame.
    let buf: Vec<f32> = (0..512)
        .map(|i| if i % 2 == 0 { 0.01 } else { -0.01 })
        .collect();
    assert_eq!(find_trigger(&buf, buf.len()), 0);
}

#[test]
fn an_empty_window_has_no_trigger() {
    assert_eq!(find_trigger(&[], 16), 0);
}

// --- min_max_columns ---------------------------------------------------------

#[test]
fn every_column_reports_the_full_range_it_covers() {
    // A bucket whose extremes lie either side of zero: the whole point of a
    // range is that neither is lost, which is what a single peak or a mean does.
    let mut src = vec![0.1; 8];
    src[2] = -0.9;
    src[5] = 0.7;
    let mut out = [Column::default(); 1];

    min_max_columns(&src, &mut out);

    approx(out[0].min, -0.9);
    approx(out[0].max, 0.7);
}

#[test]
fn columns_are_independent_of_each_other() {
    // Loud first half, quiet second half.
    let mut src = vec![0.2; 100];
    src[10] = 1.0;
    src[60] = -0.3;
    let mut out = [Column::default(); 2];

    min_max_columns(&src, &mut out);

    approx(out[0].max, 1.0);
    approx(out[1].max, 0.2);
    approx(out[1].min, -0.3);
}

#[test]
fn a_column_never_inverts() {
    // `min <= max` is what keeps the drawn figure from folding over itself.
    let mut src = vec![0.0; 997];
    fill_sine(&mut src, 31.0);
    let mut out = [Column::default(); 128];

    min_max_columns(&src, &mut out);

    for (i, c) in out.iter().enumerate() {
        assert!(c.min <= c.max, "column {i} inverted: {c:?}");
    }
}

#[test]
fn every_sample_reaches_some_column() {
    // No sample may be skipped — that is the property that makes aliasing
    // impossible, and the one a decimating reducer cannot offer.
    let mut src = vec![0.05; 1000];
    src[377] = -1.0;
    src[911] = 1.0;
    let mut out = [Column::default(); 97];

    min_max_columns(&src, &mut out);

    approx(peak(&out), 1.0);
    assert!(out.iter().any(|c| (c.min + 1.0).abs() < 1e-4), "the trough was dropped");
    assert!(out.iter().any(|c| (c.max - 1.0).abs() < 1e-4), "the peak was dropped");
}

#[test]
fn columns_hold_the_nearest_sample_when_asked_for_more_than_there_are() {
    let src = [1.0, -1.0, 0.5];
    let mut out = [Column::default(); 6];

    min_max_columns(&src, &mut out);

    // Each source sample is held across the two columns that map onto it, so a
    // column has zero height rather than being left as a gap.
    for (i, expected) in [1.0, 1.0, -1.0, -1.0, 0.5, 0.5].into_iter().enumerate() {
        approx(out[i].min, expected);
        approx(out[i].max, expected);
    }
}

#[test]
fn an_empty_source_blanks_the_columns() {
    let mut out = [Column { min: -0.7, max: 0.7 }; 4];
    min_max_columns(&[], &mut out);
    for c in out {
        approx(c.min, 0.0);
        approx(c.max, 0.0);
    }
}

#[test]
fn reducing_into_no_columns_is_a_no_op() {
    let mut out: [Column; 0] = [];
    min_max_columns(&[1.0, 2.0], &mut out);
}

// --- columns_for_width -------------------------------------------------------

#[test]
fn the_column_count_follows_the_strip_width() {
    // Two logical pixels a column, so a wider strip draws more of them.
    assert_eq!(columns_for_width(600.0), 300);
    assert_eq!(columns_for_width(400.0), 200);
    assert!(columns_for_width(700.0) > columns_for_width(300.0));
}

#[test]
fn the_column_count_is_bounded_at_both_ends() {
    assert_eq!(columns_for_width(4000.0), MAX_COLUMNS);
    assert_eq!(columns_for_width(10.0), MIN_COLUMNS);
}

#[test]
fn a_nonsense_width_still_yields_drawable_columns() {
    // The width crosses the language boundary as a float, and the strip is
    // briefly zero-sized before its first layout. Anything unusable draws the
    // cheapest trace rather than the most expensive one.
    assert_eq!(columns_for_width(0.0), MIN_COLUMNS);
    assert_eq!(columns_for_width(-50.0), MIN_COLUMNS);
    assert_eq!(columns_for_width(f32::NAN), MIN_COLUMNS);
    assert_eq!(columns_for_width(f32::INFINITY), MIN_COLUMNS);
}

// --- write_path_commands -----------------------------------------------------

#[test]
fn the_path_closes_one_figure_around_both_edges() {
    let columns = [
        Column { min: -0.5, max: 0.5 },
        Column { min: -0.25, max: 0.75 },
        Column { min: -1.0, max: 0.0 },
    ];
    let mut out = String::new();
    write_path_commands(&columns, &mut out);

    assert!(out.starts_with('M'), "path did not start with a move: {out}");
    assert!(out.ends_with('Z'), "path was left open: {out}");
    assert_eq!(out.matches('M').count(), 1);
    // One vertex per column on the way out, one on the way back, less the move.
    assert_eq!(out.matches('L').count(), columns.len() * 2 - 1);
}

#[test]
fn the_path_spans_x_from_zero_to_one_and_back() {
    let columns = [Column::default(); 4];
    let mut out = String::new();
    write_path_commands(&columns, &mut out);

    assert!(out.starts_with("M0.0000 "), "upper edge does not start at x = 0: {out}");
    assert!(out.contains("L1.0000 "), "the edges never reach x = 1: {out}");
    assert!(out.ends_with("L0.0000 0.000Z"), "lower edge does not return to x = 0: {out}");
}

#[test]
fn the_path_flips_both_edges_so_peaks_point_upward() {
    // Screen y grows downward, so a positive sample has to come out negative or
    // the whole trace draws upside down.
    let mut out = String::new();
    write_path_commands(&[Column { min: -0.25, max: 0.75 }], &mut out);

    assert_eq!(out, "M0.0000 -0.750 L0.0000 0.250Z");
}

#[test]
fn a_resting_trace_has_no_negative_zeroes() {
    // Cosmetic, but a flat line rendered as hundreds of `-0.000` vertices is the
    // sort of thing that looks like a bug in a debugger.
    let mut out = String::new();
    write_path_commands(&[Column::default(); 3], &mut out);

    assert!(!out.contains('-'), "silence formatted with a sign: {out}");
}

#[test]
fn writing_a_path_reuses_the_buffer() {
    let mut out = String::from("stale");
    write_path_commands(&[Column::default(); 2], &mut out);
    assert!(!out.contains("stale"));
}

#[test]
fn an_empty_trace_writes_an_empty_path() {
    let mut out = String::from("stale");
    write_path_commands(&[], &mut out);
    assert!(out.is_empty());
}

// --- WaveformAnalyzer --------------------------------------------------------

#[test]
fn the_window_spans_the_same_time_at_any_rate() {
    // A fixed sample count would show a 96 kHz file half as much music as a
    // 44.1 kHz one. Time, not samples.
    let analyzer = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);
    let ms = u64::from(WAVE_SPAN_MS + TRIGGER_SLACK_MS);

    for rate in [44_100_u32, 48_000, 96_000] {
        let want = usize::try_from(u64::from(rate) * ms / 1000).unwrap_or(usize::MAX);
        assert_eq!(analyzer.window_len(rate), want, "rate {rate}");
    }
}

#[test]
fn the_window_never_outruns_its_buffer() {
    // A rate high enough to want more than the ring holds narrows the trace in
    // time rather than reading past the end of the buffer.
    let analyzer = WaveformAnalyzer::new(512, MAX_COLUMNS);
    assert_eq!(analyzer.window_len(192_000), 512);
    // And a rate of zero — nothing played yet — still leaves a usable slice.
    assert!(analyzer.window_len(0) >= 2);
}

#[test]
fn the_analyzer_traces_the_window_it_was_given() {
    let mut analyzer = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);
    fill_sine(analyzer.window_mut(RATE), 16.0);

    let columns = analyzer.analyze(true, RATE, 256);

    assert_eq!(columns.len(), 256);
    assert!(
        peak(columns) > 0.9,
        "a full-scale sine should reach the top of the trace, got {}",
        peak(columns)
    );
}

#[test]
fn the_analyzer_draws_as_many_columns_as_asked_for() {
    let mut analyzer = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);
    fill_sine(analyzer.window_mut(RATE), 16.0);

    assert_eq!(analyzer.analyze(true, RATE, 128).len(), 128);
    assert_eq!(analyzer.analyze(true, RATE, 400).len(), 400);
    // Past the buffer it was built with, so it gives what it has.
    assert_eq!(analyzer.analyze(true, RATE, MAX_COLUMNS * 4).len(), MAX_COLUMNS);
    assert_eq!(analyzer.analyze(true, RATE, 0).len(), 1);
}

#[test]
fn the_analyzer_traces_the_same_shape_from_a_shifted_window() {
    // The whole point of triggering: two snapshots of the same tone taken at
    // different phases must draw the same trace.
    let mut a = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);
    let mut b = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);

    let window = a.window_len(RATE);
    let mut source = vec![0.0; window + window / 16];
    fill_sine(&mut source, 40.0);
    a.window_mut(RATE).copy_from_slice(&source[..window]);
    b.window_mut(RATE).copy_from_slice(&source[window / 48..][..window]);

    let first: Vec<Column> = a.analyze(true, RATE, 192).to_vec();
    let second = b.analyze(true, RATE, 192);

    for (i, (x, y)) in first.iter().zip(second).enumerate() {
        assert!((x.max - y.max).abs() < 0.25, "column {i} upper edge drifted: {x:?} vs {y:?}");
        assert!((x.min - y.min).abs() < 0.25, "column {i} lower edge drifted: {x:?} vs {y:?}");
    }
}

#[test]
fn an_inactive_analyzer_decays_toward_the_centre_line() {
    let mut analyzer = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);
    fill_sine(analyzer.window_mut(RATE), 16.0);
    assert!(peak(analyzer.analyze(true, RATE, 256)) > 0.9);

    // Long enough that geometric decay lands well under the caller's idle floor.
    for _ in 0..64 {
        analyzer.analyze(false, RATE, 256);
    }

    let rest = peak(analyzer.analyze(false, RATE, 256));
    assert!(rest < 0.001, "trace never settled, peak still {rest}");
}

#[test]
fn an_inactive_analyzer_does_not_reread_the_window() {
    // A pause leaves the last window of audio in the ring. Re-analysing it would
    // freeze the trace on that shape instead of letting it fall.
    let mut analyzer = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);
    fill_sine(analyzer.window_mut(RATE), 16.0);

    let before = peak(analyzer.analyze(false, RATE, 256));

    assert!(before < f32::EPSILON, "an untouched trace should start flat, got {before}");
}

#[test]
fn a_column_widened_back_into_view_while_paused_has_decayed_with_the_rest() {
    let mut analyzer = WaveformAnalyzer::new(WINDOW_CAP, MAX_COLUMNS);
    fill_sine(analyzer.window_mut(RATE), 16.0);
    assert!(peak(analyzer.analyze(true, RATE, 400)) > 0.9);

    // Narrow, decay, then widen: the columns that were out of view must not pop
    // back at full height.
    for _ in 0..64 {
        analyzer.analyze(false, RATE, 100);
    }

    let rest = peak(analyzer.analyze(false, RATE, 400));
    assert!(rest < 0.001, "a column outside the drawn width kept its height: {rest}");
}
