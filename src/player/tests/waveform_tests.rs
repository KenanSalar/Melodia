//! Tests for the visualizer's oscilloscope trace.

use super::*;

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

// --- downsample --------------------------------------------------------------

#[test]
fn downsampling_fills_every_slot() {
    let src: Vec<f32> = (0..1024).map(|i| if i % 3 == 0 { 0.5 } else { -0.25 }).collect();
    let mut out = vec![f32::NAN; WAVE_POINTS];

    downsample(&src, &mut out);

    assert!(out.iter().all(|v| v.is_finite()), "a slot was left unwritten");
}

#[test]
fn downsampling_keeps_the_peak_and_its_sign() {
    // One bucket's worth of samples, with the extreme value negative: a mean
    // would report roughly zero, a magnitude-only peak would report +0.9.
    let mut src = vec![0.1; 8];
    src[3] = -0.9;
    let mut out = [0.0; 1];

    downsample(&src, &mut out);

    approx(out[0], -0.9);
}

#[test]
fn downsampling_reports_each_bucket_separately() {
    // Loud first half, quiet second half.
    let mut src = vec![0.2; 100];
    src[10] = 1.0;
    let mut out = [0.0; 2];

    downsample(&src, &mut out);

    approx(out[0], 1.0);
    approx(out[1], 0.2);
}

#[test]
fn downsampling_holds_the_nearest_sample_when_asked_for_more_points_than_samples() {
    let src = [1.0, -1.0, 0.5];
    let mut out = [0.0; 6];

    downsample(&src, &mut out);

    // Each source sample is held across the two slots that map onto it.
    approx(out[0], 1.0);
    approx(out[1], 1.0);
    approx(out[2], -1.0);
    approx(out[3], -1.0);
    approx(out[4], 0.5);
    approx(out[5], 0.5);
}

#[test]
fn downsampling_an_empty_source_blanks_the_output() {
    let mut out = [0.7; 4];
    downsample(&[], &mut out);
    for slot in out {
        approx(slot, 0.0);
    }
}

#[test]
fn downsampling_into_an_empty_output_is_a_no_op() {
    let mut out: [f32; 0] = [];
    downsample(&[1.0, 2.0], &mut out);
}

// --- write_path_commands -----------------------------------------------------

#[test]
fn the_path_starts_with_a_move_and_continues_with_lines() {
    let mut out = String::new();
    write_path_commands(&[0.0, 0.5, -0.5, 1.0], &mut out);

    assert!(out.starts_with('M'), "path did not start with a move: {out}");
    assert_eq!(out.matches('M').count(), 1);
    assert_eq!(out.matches('L').count(), 3);
}

#[test]
fn the_path_spans_x_from_zero_to_one() {
    let mut out = String::new();
    write_path_commands(&[0.0; 5], &mut out);

    assert!(out.starts_with("M0.0000 "), "first vertex is not at x = 0: {out}");
    assert!(out.ends_with("L1.0000 0.000"), "last vertex is not at x = 1: {out}");
}

#[test]
fn the_path_flips_the_sample_so_peaks_point_upward() {
    // Screen y grows downward, so a positive sample has to come out negative or
    // the whole trace draws upside down.
    let mut out = String::new();
    write_path_commands(&[0.75, -0.25], &mut out);

    assert_eq!(out, "M0.0000 -0.750 L1.0000 0.250");
}

#[test]
fn a_resting_trace_has_no_negative_zeroes() {
    // Cosmetic, but a flat line rendered as 192 `-0.000` vertices is the sort of
    // thing that looks like a bug in a debugger.
    let mut out = String::new();
    write_path_commands(&[0.0; 3], &mut out);

    assert!(!out.contains('-'), "silence formatted with a sign: {out}");
}

#[test]
fn writing_a_path_reuses_the_buffer() {
    let mut out = String::from("stale");
    write_path_commands(&[0.0, 1.0], &mut out);
    assert!(!out.contains("stale"));
}

#[test]
fn an_empty_trace_writes_an_empty_path() {
    let mut out = String::from("stale");
    write_path_commands(&[], &mut out);
    assert!(out.is_empty());
}

#[test]
fn a_single_vertex_writes_only_a_move() {
    let mut out = String::new();
    write_path_commands(&[0.5], &mut out);
    assert_eq!(out, "M0.0000 -0.500");
}

// --- WaveformAnalyzer --------------------------------------------------------

#[test]
fn the_analyzer_traces_the_window_it_was_given() {
    let mut analyzer = WaveformAnalyzer::new(WAVE_WINDOW, WAVE_POINTS);
    fill_sine(analyzer.window_mut(), 16.0);

    let points = analyzer.analyze(true);

    assert_eq!(points.len(), WAVE_POINTS);
    let peak = points.iter().fold(0.0_f32, |m, p| m.max(p.abs()));
    assert!(peak > 0.9, "a full-scale sine should reach the top of the trace, got {peak}");
}

#[test]
fn the_analyzer_traces_the_same_shape_from_a_shifted_window() {
    // The whole point of triggering: two snapshots of the same tone taken at
    // different phases must draw the same trace.
    let mut a = WaveformAnalyzer::new(WAVE_WINDOW, WAVE_POINTS);
    let mut b = WaveformAnalyzer::new(WAVE_WINDOW, WAVE_POINTS);

    // 16 whole periods over the window, so a shift of a third of a period is a
    // pure phase offset with no discontinuity at the ends.
    let mut source = vec![0.0; WAVE_WINDOW + WAVE_WINDOW / 16];
    fill_sine(&mut source, 16.0 + 1.0);
    a.window_mut().copy_from_slice(&source[..WAVE_WINDOW]);
    b.window_mut().copy_from_slice(&source[WAVE_WINDOW / 48..][..WAVE_WINDOW]);

    let first: Vec<f32> = a.analyze(true).to_vec();
    let second = b.analyze(true);

    for (i, (&x, &y)) in first.iter().zip(second).enumerate() {
        assert!((x - y).abs() < 0.2, "vertex {i} drifted: {x} vs {y}");
    }
}

#[test]
fn an_inactive_analyzer_decays_toward_the_centre_line() {
    let mut analyzer = WaveformAnalyzer::new(WAVE_WINDOW, WAVE_POINTS);
    fill_sine(analyzer.window_mut(), 16.0);
    let peak = analyzer.analyze(true).iter().fold(0.0_f32, |m, p| m.max(p.abs()));
    assert!(peak > 0.9);

    // Long enough that geometric decay lands well under the caller's idle floor.
    for _ in 0..64 {
        analyzer.analyze(false);
    }

    let rest = analyzer.analyze(false).iter().fold(0.0_f32, |m, p| m.max(p.abs()));
    assert!(rest < 0.001, "trace never settled, peak still {rest}");
}

#[test]
fn an_inactive_analyzer_does_not_reread_the_window() {
    // A pause leaves the last window of audio in the ring. Re-analysing it would
    // freeze the trace on that shape instead of letting it fall.
    let mut analyzer = WaveformAnalyzer::new(WAVE_WINDOW, WAVE_POINTS);
    fill_sine(analyzer.window_mut(), 16.0);

    let before = analyzer.analyze(false).iter().fold(0.0_f32, |m, p| m.max(p.abs()));

    assert!(before < f32::EPSILON, "an untouched trace should start flat, got {before}");
}
