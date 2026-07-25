//! Tests for the visualizer's spectrum analysis.

use super::*;
use crate::player::dsp::db_to_linear;

// --- helpers -----------------------------------------------------------------

/// Assert two values are equal to within a tight tolerance.
fn approx(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
}

/// Fill a buffer with a full-scale sine.
#[allow(
    clippy::cast_precision_loss,
    reason = "test buffers are a few thousand samples, which convert to f32 exactly"
)]
fn fill_sine(buf: &mut [f32], freq_hz: f32, sample_rate: f32) {
    for (i, sample) in buf.iter_mut().enumerate() {
        *sample = (2.0 * std::f32::consts::PI * freq_hz * i as f32 / sample_rate).sin();
    }
}

/// Index and value of the loudest band.
fn loudest(levels: &[f32]) -> (usize, f32) {
    levels
        .iter()
        .enumerate()
        .fold((0, f32::MIN), |best, (i, &v)| if v > best.1 { (i, v) } else { best })
}

// --- hann_window -------------------------------------------------------------

#[test]
fn the_hann_window_starts_and_ends_at_zero() {
    let w = hann_window(FFT_SIZE);
    assert_eq!(w.len(), FFT_SIZE);
    approx(w[0], 0.0);
    approx(w[FFT_SIZE - 1], 0.0);
}

#[test]
fn the_hann_window_peaks_at_one_in_the_middle() {
    // An odd length puts a sample exactly on the peak.
    let odd = hann_window(9);
    approx(odd[4], 1.0);
    // An even length straddles it, so the two centre samples are both ~1.
    let w = hann_window(FFT_SIZE);
    approx(w[FFT_SIZE / 2], 1.0);
    approx(w[FFT_SIZE / 2 - 1], 1.0);
}

#[test]
fn the_hann_window_is_symmetric() {
    let w = hann_window(FFT_SIZE);
    for (i, &v) in w.iter().enumerate() {
        approx(v, w[FFT_SIZE - 1 - i]);
    }
}

#[test]
fn a_degenerate_hann_window_is_a_passthrough() {
    // No span to taper across — the formula would divide by zero.
    assert!(hann_window(0).is_empty());
    assert_eq!(hann_window(1).len(), 1);
    approx(hann_window(1)[0], 1.0);
}

// --- coherent_gain_scale -----------------------------------------------------

#[test]
fn the_hann_windows_coherent_gain_is_one_half() {
    let w = hann_window(FFT_SIZE);
    let mean = w.iter().sum::<f32>() / index_to_f32(FFT_SIZE);
    assert!((mean - 0.5).abs() < 1e-3, "coherent gain should be ~0.5, got {mean}");
    // The scale is what turns `A/2 · Σw` back into `A`.
    approx(coherent_gain_scale(&w) * w.iter().sum::<f32>(), 2.0);
}

#[test]
fn an_empty_window_has_no_scale() {
    approx(coherent_gain_scale(&[]), 0.0);
}

// --- band_bins ---------------------------------------------------------------

const RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

#[test]
fn every_band_gets_at_least_one_bin() {
    for rate in RATES {
        let map = band_bins(NUM_BANDS, FFT_SIZE, rate);
        assert_eq!(map.len(), NUM_BANDS, "at {rate} Hz");
        for (i, range) in map.iter().enumerate() {
            assert!(range.end > range.start, "band {i} is empty at {rate} Hz");
        }
    }
}

#[test]
fn bands_are_contiguous_and_monotonic() {
    for rate in RATES {
        let map = band_bins(NUM_BANDS, FFT_SIZE, rate);
        // No band starts on DC, and each one picks up where the last left off.
        assert!(map.first().is_some_and(|r| r.start >= 1), "band 0 includes DC at {rate} Hz");
        for pair in map.windows(2) {
            if let [prev, next] = pair {
                assert_eq!(prev.end, next.start, "gap or overlap at {rate} Hz");
            }
        }
    }
}

#[test]
fn no_band_reaches_past_the_nyquist_bin() {
    let bin_count = FFT_SIZE / 2 + 1;
    for rate in RATES {
        let map = band_bins(NUM_BANDS, FFT_SIZE, rate);
        for range in &map {
            assert!(range.end <= bin_count, "band ends at {} past {bin_count}", range.end);
        }
        // The top band should actually reach the top — otherwise treble is lost.
        assert!(map.last().is_some_and(|r| r.end == bin_count), "at {rate} Hz");
    }
}

#[test]
fn a_nonsense_sample_rate_yields_an_empty_map() {
    for rate in [0.0, -44_100.0, f32::NAN, f32::INFINITY, 30.0] {
        assert!(band_bins(NUM_BANDS, FFT_SIZE, rate).is_empty(), "rate {rate}");
    }
    assert!(band_bins(0, FFT_SIZE, 44_100.0).is_empty());
    assert!(band_bins(NUM_BANDS, 1, 44_100.0).is_empty());
}

#[test]
fn more_bands_than_bins_leaves_the_tail_empty_but_valid() {
    // A 16-point transform has 9 bins, so 32 bands cannot each have one.
    let map = band_bins(NUM_BANDS, 16, 44_100.0);
    assert_eq!(map.len(), NUM_BANDS);
    for range in &map {
        assert!(range.start <= range.end, "range {range:?} is inverted");
        assert!(range.end <= 9, "range {range:?} runs past the spectrum");
    }
}

// --- dsp::linear_to_db -------------------------------------------------------

#[test]
fn linear_to_db_reference_points() {
    // The inverse of `db_to_linear`, which `replaygain_tests` pins from the
    // other side: unity is 0 dB, ×2 is ~+6.02, ×0.5 is ~-6.02, ×10 is +20.
    approx(linear_to_db(1.0), 0.0);
    approx(linear_to_db(2.0), 6.020_6);
    approx(linear_to_db(0.5), -6.020_6);
    approx(linear_to_db(10.0), 20.0);
}

// --- level_from_magnitude ----------------------------------------------------

#[test]
fn silence_maps_to_a_zero_level() {
    // The guard that keeps `log10(0)` out of the bars.
    approx(level_from_magnitude(0.0), 0.0);
    approx(level_from_magnitude(-1.0), 0.0);
}

#[test]
fn full_scale_maps_to_a_full_level() {
    approx(level_from_magnitude(1.0), 1.0);
    // Anything louder clamps rather than overshooting the bar.
    approx(level_from_magnitude(4.0), 1.0);
}

#[test]
fn the_floor_maps_to_a_zero_level() {
    approx(level_from_magnitude(db_to_linear(FLOOR_DB)), 0.0);
    approx(level_from_magnitude(db_to_linear(FLOOR_DB / 2.0)), 0.5);
}

// --- bands_from_spectrum -----------------------------------------------------

#[test]
fn a_band_takes_its_loudest_bin() {
    let spectrum = [
        Complex::new(0.0, 0.0),
        Complex::new(0.25, 0.0),
        Complex::new(1.0, 0.0),
        Complex::new(0.0, 0.0),
        Complex::new(0.0, 0.0),
        Complex::new(0.0, 0.0),
    ];
    let map = [1..3, 3..6];
    let mut out = [0.0; 2];
    bands_from_spectrum(&spectrum, &map, 1.0, &mut out);
    // The 1.0 bin wins over its quieter neighbour, so the bar is full...
    approx(out[0], 1.0);
    // ...and it stays out of the next band.
    approx(out[1], 0.0);
}

#[test]
fn bands_beyond_the_map_read_as_silence() {
    let spectrum = [Complex::new(1.0, 0.0); 4];
    let mut out = [0.5; 4];
    bands_from_spectrum(&spectrum, &[0..2, 2..4], 1.0, &mut out);
    approx(out[0], 1.0);
    approx(out[1], 1.0);
    // Stale heights left in the tail would freeze on screen.
    approx(out[2], 0.0);
    approx(out[3], 0.0);
}

// --- smooth ------------------------------------------------------------------

#[test]
fn levels_rise_immediately_to_a_higher_value() {
    let mut levels = [0.0, 0.2];
    smooth(&mut levels, &[1.0, 0.9], ATTACK, DECAY);
    approx(levels[0], 1.0);
    approx(levels[1], 0.9);
}

#[test]
fn levels_decay_gradually_toward_a_lower_value() {
    let mut levels = [1.0];
    let mut previous = 1.0;
    for _ in 0..8 {
        smooth(&mut levels, &[0.0], ATTACK, DECAY);
        assert!(levels[0] < previous, "level should fall, went {previous} -> {}", levels[0]);
        previous = levels[0];
    }
    // Gradually: a single frame is nowhere near the whole distance.
    assert!(previous > 0.0, "eight frames should not have reached silence");
}

#[test]
fn decayed_levels_converge_to_zero() {
    let mut levels = [1.0];
    for _ in 0..200 {
        smooth(&mut levels, &[0.0], ATTACK, DECAY);
    }
    approx(levels[0], 0.0);
}

#[test]
fn a_level_never_decays_below_its_band() {
    let mut levels = [1.0];
    for _ in 0..50 {
        smooth(&mut levels, &[0.4], ATTACK, DECAY);
        assert!(levels[0] >= 0.4, "decayed past the band's own level: {}", levels[0]);
    }
    approx(levels[0], 0.4);
}

// --- SpectrumAnalyzer --------------------------------------------------------

#[test]
fn a_full_scale_sine_lands_in_the_band_containing_its_frequency() {
    let rate = 44_100;
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_sine(analyzer.window_mut(), 1_000.0, rate_to_f32(rate));
    let levels = analyzer.analyze(rate);

    let (peak_band, peak_level) = loudest(levels);
    let tone_bin = hz_to_bin(1_000.0, FFT_SIZE, rate_to_f32(rate), FFT_SIZE / 2);
    let map = band_bins(NUM_BANDS, FFT_SIZE, rate_to_f32(rate));
    assert_eq!(map.iter().position(|r| r.contains(&tone_bin)), Some(peak_band));

    // Full scale reads near the top of the bar; Hann's worst-case scalloping
    // loss is ~1.4 dB, which is a hair off full height on a 70 dB scale.
    assert!(peak_level > 0.95, "expected a near-full bar, got {peak_level}");
    // ...and the rest of the display stays down.
    assert!(levels[0] < 0.2, "bass leaked from a 1 kHz tone: {}", levels[0]);
}

#[test]
fn silence_produces_all_zero_bands() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    analyzer.window_mut().fill(0.0);
    let levels = analyzer.analyze(44_100);
    assert_eq!(levels.len(), NUM_BANDS);
    for (i, &level) in levels.iter().enumerate() {
        assert!(level.abs() < 1e-4, "band {i} is {level} on silence");
    }
}

#[test]
fn an_unknown_sample_rate_produces_no_bands() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_sine(analyzer.window_mut(), 1_000.0, 44_100.0);
    // Nothing has played, so there is no rate to place band edges against.
    let levels = analyzer.analyze(0);
    for (i, &level) in levels.iter().enumerate() {
        assert!(level.abs() < 1e-4, "band {i} is {level} with no sample rate");
    }
}

#[test]
fn the_bars_decay_once_the_signal_stops() {
    let rate = 44_100;
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_sine(analyzer.window_mut(), 1_000.0, rate_to_f32(rate));
    let loud = loudest(analyzer.analyze(rate));

    analyzer.window_mut().fill(0.0);
    let quiet = analyzer.analyze(rate);
    assert!(quiet[loud.0] < loud.1, "the bar should start falling once the tone stops");
    assert!(quiet[loud.0] > 0.0, "it should fall gently, not cut out");
}

#[test]
fn changing_the_sample_rate_rebuilds_the_band_map() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    analyzer.window_mut().fill(0.0);
    analyzer.analyze(44_100);
    assert_eq!(analyzer.mapped_rate, 44_100);
    let first: Vec<_> = analyzer.map.to_vec();

    analyzer.window_mut().fill(0.0);
    analyzer.analyze(96_000);
    assert_eq!(analyzer.mapped_rate, 96_000);
    assert_ne!(analyzer.map.to_vec(), first, "band edges must follow the sample rate");
}
